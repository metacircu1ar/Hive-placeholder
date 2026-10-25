//! Durable human approval creation and response transitions.
use super::{AgentState, Caller, Core, EventKind, api, eligible};
use crate::runtime::api::events;
use crate::{
    approvals::{self, Approval, ApprovalAction, ApprovalState, AskResponse, RespondResponse},
    error::{Error, Result},
    runtime::api::Input,
    state,
};
use api::ApprovalInput;
use hive_input::Code;
use serde_json::Value;
use uuid::Uuid;

impl Core {
    pub(super) fn ask(&mut self, caller: &Caller, args: &api::AskArgs) -> Result<Value> {
        if caller.agent_id().is_none() {
            return Err(Error::unsupported_operation(
                "Only an agent can request human approval.",
            ));
        }
        let agent = self.store.agent(caller.name())?;
        let (Some(execution), Some(turn)) = (&agent.execution_id, &agent.turn_id) else {
            return Err(Error::no_active_turn(
                "Approval requests require an active provider turn.",
            ));
        };
        if agent.state != AgentState::Running {
            return Err(Error::no_active_turn(
                "Approval requests require an active provider turn.",
            ));
        }
        let api::AskContent {
            message,
            title,
            schema,
            destructive,
        } = args.content()?;
        let approval = Approval {
            id: Uuid::new_v4().to_string(),
            agent_id: agent.id.clone(),
            execution: execution.clone(),
            turn: turn.clone(),
            title,
            message: message.into(),
            schema,
            destructive,
            state: ApprovalState::Pending,
            action: None,
            content: None,
            reason: None,
            created_at: state::now(),
            closed_at: None,
            response_message: None,
        };
        self.store
            .close_pending_approval(&agent, ApprovalState::Superseded)?;
        self.store.insert_approval(&approval)?;
        self.store.event(
            &agent,
            EventKind::ApprovalRequested,
            api::value(events::ApprovalRequested {
                approval_id: &approval.id,
                title: approval.title.as_deref(),
                destructive,
                state: ApprovalState::Pending,
            }),
        )?;
        Ok(api::value(AskResponse {
            approval_id: approval.id,
            state: ApprovalState::Pending,
        }))
    }

    pub(super) fn respond(&mut self, args: &api::RespondArgs) -> Result<Value> {
        let approval = self
            .store
            .approval(ApprovalInput::Respond.id(&args.approval_id)?)?;
        if approval.state != ApprovalState::Pending {
            return Err(Error::approval_closed(
                "This approval is no longer pending.",
            ));
        }
        let action = args.action.copied().ok_or_else(|| {
            ApprovalInput::Respond.error(
                if args.action.is_missing() {
                    Code::MissingArgument
                } else {
                    Code::InvalidValue
                },
                "/action",
                "accept, decline or cancel",
            )
        })?;
        let content = response_content(&approval, action, &args.content)?;
        let reason = response_reason(action, &args.reason)?;
        let body = answer_body(&approval, action, content, reason)?;
        let mut agent = self.store.agent(&approval.agent_id)?;
        eligible(&agent)?;
        let message_id = Uuid::new_v4().to_string();
        self.store
            .answer_approval(&approval.id, action, content, reason, &message_id)?;
        if agent.state == AgentState::Stopped && !agent.paused {
            agent.state = AgentState::Starting;
            self.store.save(&agent)?;
        }
        self.store
            .enqueue_approval_answer(&message_id, &agent.id, &body)?;
        self.store.event(
            &agent,
            EventKind::ApprovalClosed,
            api::value(events::ApprovalClosed {
                origin: Some(events::ApprovalOrigin {
                    execution: &approval.execution,
                    turn: &approval.turn,
                }),
                approval_id: &approval.id,
                state: ApprovalState::Answered,
                action: Some(action),
                response_message: Some(&message_id),
            }),
        )?;
        self.store.event(
            &agent,
            EventKind::MessageAccepted,
            api::value(events::ApprovalMessage {
                id: &message_id,
                sender: &Caller::Human,
                body: &body,
                sender_turn_id: None,
                kind: crate::runtime::domain::MessageKind::ApprovalAnswer,
                approval_id: &approval.id,
            }),
        )?;
        Ok(api::value(RespondResponse {
            approval_id: approval.id,
            state: ApprovalState::Answered,
            action,
            message_id,
            recipient: agent.id,
        }))
    }
}

fn response_reason(action: ApprovalAction, reason: &Input<String>) -> Result<Option<&str>> {
    match reason {
        Input::Missing => Ok(None),
        Input::Valid(value)
            if action == ApprovalAction::Decline && value.len() <= approvals::RESPONSE_BYTES =>
        {
            Ok(Some(value.as_str()))
        }
        Input::Valid(_) if action == ApprovalAction::Decline => Err(ApprovalInput::Respond.error(
            Code::OutOfRange,
            "/reason",
            "at most 61440 UTF-8 bytes",
        )),
        Input::Invalid(_) if action == ApprovalAction::Decline => {
            Err(ApprovalInput::Respond.error(Code::InvalidValue, "/reason", "a string reason"))
        }
        _ => Err(ApprovalInput::Respond.error(
            Code::ConflictingArguments,
            "/reason",
            "a string reason only when action is decline",
        )),
    }
}

fn response_content<'a>(
    approval: &Approval,
    action: ApprovalAction,
    content: &'a Input<Value>,
) -> Result<Option<&'a Value>> {
    if action != ApprovalAction::Accept {
        return if content.is_missing() {
            Ok(None)
        } else {
            Err(ApprovalInput::Respond.error(
                Code::ConflictingArguments,
                "/content",
                "omit content unless action is accept",
            ))
        };
    }
    match (&approval.schema, content) {
        (None, Input::Missing) => Ok(None),
        (Some(schema), Input::Valid(value)) => {
            if !approvals::valid_schema(schema) {
                return Err(Error::database_error("Invalid stored approval schema."));
            }
            if value.to_string().len() > approvals::RESPONSE_BYTES {
                return Err(ApprovalInput::Respond.error(
                    Code::OutOfRange,
                    "/content",
                    "at most 61440 serialized JSON bytes",
                ));
            }
            if approvals::valid_response(schema, value) {
                Ok(Some(value))
            } else {
                Err(ApprovalInput::Respond.error(
                    Code::InvalidValue,
                    "/content",
                    "an object matching the pending approval form",
                ))
            }
        }
        (Some(_), Input::Missing) => Err(ApprovalInput::Respond.error(
            Code::MissingArgument,
            "/content",
            "form data supplied with --data or --data-file",
        )),
        _ => Err(ApprovalInput::Respond.error(
            Code::ConflictingArguments,
            "/content",
            "omit content when the approval has no form",
        )),
    }
}

fn answer_body(
    approval: &Approval,
    action: ApprovalAction,
    content: Option<&Value>,
    reason: Option<&str>,
) -> Result<String> {
    let label = approval
        .title
        .as_ref()
        .map_or_else(String::new, |title| format!(" ({})", api::value(title)));
    let mut body = format!("Approval {}{label}: {}", approval.id, action.as_str());
    if let Some(content) = content {
        body.push('\n');
        body.push_str(&content.to_string());
    }
    if let Some(reason) = reason {
        body.push_str("\nReason: ");
        body.push_str(reason);
    }
    if body.len() > approvals::BODY_BYTES {
        return Err(ApprovalInput::Respond.error(
            Code::OutOfRange,
            if content.is_some() {
                "/content"
            } else {
                "/reason"
            },
            "a complete formatted answer within 65536 UTF-8 bytes",
        ));
    }
    Ok(body)
}
