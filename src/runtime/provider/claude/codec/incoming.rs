//! Decode Claude frames and reconcile user, system, and result evidence.
use super::limits::{CANCELLED_CALLBACK_LIMIT, TOOL_ID_BYTES};
use super::wire::emitted;
use super::{Codec, ProviderTurnStatus, Result, ServerMethod, Value, parse, protocol, wire};
use crate::runtime::api;
use crate::runtime::provider::protocol::codes;
use std::mem;

struct ResultFrame {
    session_id: String,
    subtype: wire::ResultSubtype,
    is_error: bool,
    user_message_uuid: Option<String>,
    terminal_reason: Option<wire::TerminalReason>,
    origin: Option<wire::Origin>,
}

impl Codec {
    pub(super) fn user(
        &mut self,
        uuid: Option<String>,
        session: &str,
        parent: Option<String>,
        replay: bool,
        message: Option<wire::UserMessage>,
    ) -> Result<()> {
        // A fork's prompt is never this turn's acknowledgement.
        if replay && self.in_fork(parent.as_deref()) {
            return self.check_session(session);
        }
        if replay {
            return self.acknowledge(uuid, session, parent);
        }
        self.check_session(session)?;
        // A fork's prompt and tool results arrive under its Skill call; their
        // tool results settle like the agent's own.
        if parent.is_some() && !self.in_fork(parent.as_deref()) {
            return Err(protocol());
        }
        if let Some(message) = message {
            for block in message.content.blocks {
                if block.tool_result() {
                    let id = block.tool_use_id().ok_or_else(protocol)?;
                    self.tools.active.remove(id);
                    if parent.is_none() {
                        self.settle_fork(id);
                    }
                    if let Some(permit) = self.tools.shell_permits.remove(id) {
                        self.guard.revoke(&permit.token);
                    }
                    let turn = self.active_id()?;
                    self.notice(
                        ServerMethod::ItemCompleted,
                        api::value(emitted::ItemParams {
                            thread_id: &self.config.id,
                            turn_id: &turn,
                            item: emitted::ToolResult {
                                kind: "dynamicToolCall",
                                id,
                                result: &block.raw,
                            },
                        }),
                    );
                }
            }
        }
        Ok(())
    }
    pub(crate) fn receive(&mut self, value: Value) -> Result<Vec<Value>> {
        // Capture execution evidence before typed parsing can fail. Initialization
        // metadata and exact transport echoes cannot execute an agent task.
        let metadata = wire::PreinputMetadata::read(&value);
        if !self.turn.input.enqueued && !metadata.allowed() {
            self.turn.input.execution_observed = true;
            return Err(protocol());
        }
        if metadata.kind == Some(wire::FrameKind::ControlResponse) && self.steering_echo(&value) {
            self.echoes.pending.retain(|reply| reply != &value);
            return Ok(vec![]);
        }
        if metadata.kind == Some(wire::FrameKind::ControlResponse) {
            let echo = self.echoes.pending.iter().position(|reply| reply == &value);
            if let Some(index) = echo {
                self.echoes.pending.remove(index);
                return Ok(vec![]);
            }
        }
        if self.hold_branch_frame(&value)? {
            return Ok(vec![]);
        }
        let output = self.receive_inner(value)?;
        self.remember_replies(&output)?;
        Ok(output)
    }
    // Keep one dispatch over the complete typed frame; each branch retains its
    // payload ownership until its dedicated handler has settled.
    pub(super) fn receive_inner(&mut self, value: Value) -> Result<Vec<Value>> {
        match parse::<wire::Frame>(value)? {
            wire::Frame::ControlResponse { response } => self.control_result(response)?,
            wire::Frame::ControlRequest {
                request_id,
                request,
            } => return self.incoming_control(request_id, request),
            wire::Frame::ControlCancelRequest { request_id } => {
                self.expire_steering(false, Some(&request_id));
                self.echoes
                    .pending
                    .retain(|frame| frame["response"]["request_id"] != request_id);
                self.cancel_callback(&request_id)?;
            }
            wire::Frame::System { subtype, fields } => self.system(subtype, fields)?,
            wire::Frame::User {
                uuid,
                session_id,
                parent_tool_use_id,
                replay,
                message,
            } => self.user(uuid, &session_id, parent_tool_use_id, replay, message)?,
            wire::Frame::Assistant {
                error,
                message,
                session_id,
                parent_tool_use_id,
            } => {
                self.assistant_error(error, &session_id, parent_tool_use_id.as_deref());
                self.assistant(message, &session_id, parent_tool_use_id)?;
            }
            wire::Frame::StreamEvent {
                session_id,
                parent_tool_use_id,
                user_message_uuid,
                event,
            } => self.stream(&session_id, parent_tool_use_id, user_message_uuid, event)?,
            wire::Frame::Result {
                session_id,
                subtype,
                is_error,
                user_message_uuid,
                terminal_reason,
                _num_turns: _,
                origin,
            } => {
                self.result_frame(ResultFrame {
                    session_id,
                    subtype,
                    is_error,
                    user_message_uuid,
                    terminal_reason,
                    origin,
                })?;
            }
            wire::Frame::RateLimitEvent {
                session_id,
                rate_limit_info,
            } => {
                if session_id == self.config.id {
                    // Project in the shard; no raw quota snapshot is ever persisted.
                    self.notice(
                        ServerMethod::ClaudeRateLimits,
                        api::value(emitted::RateLimits {
                            info: rate_limit_info,
                            thread_id: &session_id,
                            turn_id: self.turn.active.as_ref().map(|t| t.id.as_str()),
                        }),
                    );
                }
            }
            wire::Frame::Other => {}
        }
        Ok(vec![])
    }
    fn assistant_error(
        &mut self,
        error: Option<wire::AssistantError>,
        session_id: &str,
        parent_tool_use_id: Option<&str>,
    ) {
        if session_id == self.config.id && parent_tool_use_id.is_none() {
            let kind = match error {
                Some(wire::AssistantError::AuthenticationFailed) => Some("unauthorized"),
                Some(wire::AssistantError::BillingError) => Some("billingError"),
                Some(wire::AssistantError::RateLimit) => Some("usageLimitExceeded"),
                _ => None,
            };
            if let Some(kind) = kind {
                self.notice(
                    ServerMethod::Error,
                    api::value(emitted::ProviderError {
                        thread_id: session_id,
                        turn_id: self.turn.active.as_ref().map(|t| t.id.as_str()),
                        will_retry: false,
                        error: emitted::ErrorInfo {
                            codex_error_info: kind,
                        },
                    }),
                );
            }
        }
    }
    fn result_frame(&mut self, frame: ResultFrame) -> Result<()> {
        let ResultFrame {
            session_id,
            subtype,
            is_error,
            user_message_uuid,
            terminal_reason,
            origin,
        } = frame;
        if origin.is_some_and(|origin| origin.kind != wire::OriginKind::Human)
            || terminal_reason == Some(wire::TerminalReason::BackgroundRequested)
        {
            self.turn.input.execution_observed = true;
            return Err(protocol());
        }
        // receive() already validated startup-rejection metadata before typed parsing.
        if !self.turn.input.enqueued
            && self.turn.active.is_none()
            && self.tools.active.is_empty()
            && self.callbacks.requests.is_empty()
        {
            self.turn.input.rejected = true;
            for (_, (id, _)) in mem::take(&mut self.callbacks.controls) {
                self.frames.push_back(emitted::error(
                    id,
                    codes::SERVER_ERROR,
                    "Claude rejected session startup.",
                ));
            }
        } else {
            self.result(
                &session_id,
                subtype,
                is_error,
                user_message_uuid.as_deref(),
                terminal_reason,
            )?;
        }
        Ok(())
    }
    pub(super) fn cancel_callback(&mut self, request_id: &str) -> Result<()> {
        // Native cancellation does not undo an already-dispatched tool.
        // A late result is discarded using its bounded tombstone.
        for id in self.callbacks.matching_requests(|id, _| id == request_id) {
            self.callbacks.requests.remove(&id);
            self.callbacks.cancelled.insert(id);
            if self.callbacks.cancelled.len() > CANCELLED_CALLBACK_LIMIT {
                return Err(protocol());
            }
        }
        Ok(())
    }
    pub(super) fn system(
        &mut self,
        subtype: wire::SystemSubtype,
        fields: wire::System,
    ) -> Result<()> {
        if subtype == wire::SystemSubtype::Init {
            self.check_session(fields.session_id.as_deref().ok_or_else(protocol)?)?;
            self.check_model(fields.model.as_deref().ok_or_else(protocol)?)?;
            if fields.permission_mode != Some(wire::PermissionMode::Default) {
                return Err(protocol());
            }
            self.verified = true;
        } else if subtype == wire::SystemSubtype::TaskStarted {
            self.open_fork(&fields)?;
        } else if matches!(
            subtype,
            wire::SystemSubtype::TaskNotification
                | wire::SystemSubtype::TaskProgress
                | wire::SystemSubtype::TaskUpdated
        ) {
            self.fork_task(subtype, &fields)?;
        } else if subtype == wire::SystemSubtype::BackgroundTasksChanged {
            // Background work is disabled. Its appearance violates quiescence.
            return Err(protocol());
        }
        Ok(())
    }
    /// A task is accepted only as a skill forked inside the active turn's
    /// allowed `Skill` call, in the foreground; any other task, including
    /// background work, violates quiescence.
    fn open_fork(&mut self, fields: &wire::System) -> Result<()> {
        let turn = self.active_id()?;
        let tool = fields.tool_use_id.as_deref().ok_or_else(protocol)?;
        let task = fields
            .task_id
            .as_deref()
            .filter(|task| !task.is_empty() && task.len() <= TOOL_ID_BYTES)
            .ok_or_else(protocol)?;
        let skill = self
            .tools
            .owners
            .get(tool)
            .is_some_and(|owner| owner.turn == turn && owner.allowed && owner.name == "Skill");
        if !self.config.skills_enabled()
            || !skill
            || !self.tools.active.contains(tool)
            || fields.is_backgrounded != Some(false)
            || self.tools.fork.is_some()
        {
            return Err(protocol());
        }
        self.tools.fork = Some(super::Fork {
            tool: tool.to_owned(),
            task: task.to_owned(),
        });
        Ok(())
    }
    /// Progress and the notification for the open fork are accepted; the fork
    /// itself ends when its `Skill` call settles.
    fn fork_task(&self, subtype: wire::SystemSubtype, fields: &wire::System) -> Result<()> {
        if self.tools.fork.is_none()
            && self.tools.settled_task.is_some()
            && fields.task_id == self.tools.settled_task
        {
            return Ok(());
        }
        let fork = self.tools.fork.as_ref().ok_or_else(protocol)?;
        if fields.task_id.as_deref() != Some(fork.task.as_str())
            || (subtype == wire::SystemSubtype::TaskNotification
                && fields.tool_use_id.as_deref() != Some(fork.tool.as_str()))
        {
            return Err(protocol());
        }
        Ok(())
    }
    /// The fork's `Skill` call settled (its result or its finished hook).
    pub(super) fn settle_fork(&mut self, tool: &str) {
        if self
            .tools
            .fork
            .as_ref()
            .is_some_and(|fork| fork.tool == tool)
        {
            self.tools.settled_task = self.tools.fork.take().map(|fork| fork.task);
        }
    }
    /// The frame belongs to the open fork's `Skill` call.
    pub(super) fn in_fork(&self, parent: Option<&str>) -> bool {
        parent.is_some_and(|parent| {
            self.tools
                .fork
                .as_ref()
                .is_some_and(|fork| fork.tool == parent)
        })
    }
    pub(super) fn result(
        &mut self,
        session: &str,
        subtype: wire::ResultSubtype,
        error: bool,
        uuid: Option<&str>,
        reason: Option<wire::TerminalReason>,
    ) -> Result<()> {
        self.check_session(session)?;
        if self.turn.completion.is_some() {
            return Err(protocol());
        }
        let turn = self.turn.active.as_ref().ok_or_else(protocol)?;
        if uuid.is_some_and(|id| id != turn.id)
            || (uuid.is_none() && !turn.acknowledged)
            || !self.callbacks.requests.is_empty()
            || !self.tools.active.is_empty()
            || self.tools.fork.is_some()
        {
            return Err(protocol());
        }
        self.turn.terminal_error = error;
        if !turn.acknowledged {
            if !error {
                return Err(protocol());
            }
            self.frames.push_back(emitted::error(
                turn.request,
                codes::SERVER_ERROR,
                "Claude rejected this input.",
            ));
            self.turn.active = None;
            return Ok(());
        }
        let status = if subtype == wire::ResultSubtype::Success
            && !error
            && reason == Some(wire::TerminalReason::Completed)
        {
            ProviderTurnStatus::Completed
        } else if matches!(
            reason,
            Some(wire::TerminalReason::AbortedStreaming | wire::TerminalReason::AbortedTools)
        ) || turn.interrupt_sent
        {
            ProviderTurnStatus::Interrupted
        } else {
            ProviderTurnStatus::Failed
        };
        let id = turn.id.clone();
        self.flush_assistant_text();
        if status == ProviderTurnStatus::Completed {
            self.steering_success();
        }
        self.expire_steering(true, None);
        self.turn.completion = Some(api::value(emitted::ThreadTurn {
            thread_id: &self.config.id,
            turn: emitted::TerminalTurn { id: &id, status },
        }));
        self.settle();
        Ok(())
    }
}
