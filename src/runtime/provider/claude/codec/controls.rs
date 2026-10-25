//! Validate native Claude tool callbacks and synthesize control replies.
use super::super::{launch::Shell, read_only_git};
use super::limits::{
    ACTIVE_TOOL_LIMIT, CANARY_TOOL_LIMIT, CANCELLED_CALLBACK_LIMIT, MCP_CALLBACK_LIMIT,
};
use super::wire::{control, mcp};
use super::{Codec, parse, protocol, wire};
use crate::error::Error;
use crate::runtime::provider::protocol::ServerMethod;
use crate::runtime::provider::protocol::codes;
use crate::runtime::provider::protocol::outgoing::ToolResponse;
use crate::shell::TOKEN_PREFIX;
use crate::{error::Result, runtime::api};
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;

fn denied(name: &str, phase: &'static str) {
    static DENIED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    // Only fixed native names enter logs; MCP and unknown names can contain private text.
    let method = match name {
        "Read" | "Glob" | "Grep" | "Bash" | "Edit" | "Write" | "NotebookEdit" | "Skill" => name,
        _ => "other",
    };
    if let Some(count) = crate::observability::sample(&DENIED) {
        tracing::info!(name: "tool_policy.denied",
             provider = "claude",
             method = method,
             phase = phase,
             count = count);
    }
}

impl Codec {
    #[cfg(feature = "comb")]
    pub(crate) fn forget_tool(&mut self, id: &Value) -> Result<()> {
        let id = id.as_u64().ok_or_else(protocol)?;
        self.callbacks.requests.remove(&id);
        self.callbacks.cancelled.remove(&id);
        #[cfg(feature = "comb")]
        self.callbacks.comb_turns.remove(&id);
        Ok(())
    }
    #[cfg(feature = "comb")]
    pub(crate) fn forget_comb_turn(&mut self, turn: &str) {
        self.callbacks.retired_comb_turn = Some(turn.into());
        let ids: Vec<_> = self
            .callbacks
            .comb_turns
            .iter()
            .filter(|(_, owner)| *owner == turn)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.callbacks.requests.remove(&id);
            self.callbacks.cancelled.remove(&id);
            #[cfg(feature = "comb")]
            self.callbacks.comb_turns.remove(&id);
        }
    }
    pub(super) fn incoming_control(
        &mut self,
        id: String,
        request: wire::ControlRequest,
    ) -> Result<Vec<Value>> {
        let payload = match request {
            wire::ControlRequest::McpMessage {
                server_name,
                message,
            } => {
                if server_name != wire::McpServer::Hive {
                    return Err(protocol());
                }
                return self.mcp(id, message);
            }
            wire::ControlRequest::HookCallback { callback_id, input } => {
                self.hook(callback_id, input)?
            }
            wire::ControlRequest::CanUseTool {
                tool_name,
                input,
                decision_reason,
                blocked_path,
                tool_use_id,
            } => {
                let allowed = if self.guard.sealed() {
                    false
                } else if wire::NativeTool::parse(&tool_name) == wire::NativeTool::Bash {
                    self.shell_permission(tool_use_id.as_deref(), &input)
                } else {
                    self.allowed(&tool_name, &input)
                        .inspect_err(|error| {
                            crate::observability::observation_failed("claude_tool_policy", error);
                        })
                        .unwrap_or(false)
                };
                if allowed
                    && (self.config.shell() == Shell::Unrestricted
                        || (decision_reason.is_none() && blocked_path.is_none()))
                {
                    api::value(control::Permission::Allow { input })
                } else {
                    denied(&tool_name, "permission");
                    api::value(control::Permission::Deny {
                        message: "Hive policy does not allow this tool input.",
                    })
                }
            }
            wire::ControlRequest::Other => {
                return Ok(vec![control::unsupported(&id)]);
            }
        };
        let mut reply = control::reply(&id, payload);
        self.emitted_guidance(&mut reply)?;
        Ok(vec![reply])
    }
    fn mcp(&mut self, control: String, request: wire::McpRequest) -> Result<Vec<Value>> {
        let result = match request.method {
            wire::McpMethod::Initialize => {
                let Some(protocol) = mcp::initialize_protocol(&request.params) else {
                    return Ok(vec![mcp::error(
                        &control,
                        request.id.as_ref(),
                        codes::INVALID_PARAMS,
                        "Invalid MCP protocol version.",
                    )]);
                };
                mcp::ResultBody::initialize(protocol)
            }
            wire::McpMethod::Initialized | wire::McpMethod::Ping => mcp::ResultBody::empty(),
            wire::McpMethod::Cancelled => {
                self.cancel_mcp(request.params)?;
                mcp::ResultBody::empty()
            }
            wire::McpMethod::ToolsList => {
                let tools =
                    Vec::<mcp::Definition>::deserialize(crate::runtime::tool_registry::tools(
                        self.config.tool_catalog,
                        self.config.comb_reviewer,
                    ))
                    .map_err(|cause| protocol().with_source(cause))?;
                mcp::ResultBody::Catalog(mcp::Catalog { tools })
            }
            wire::McpMethod::ToolsCall => return self.mcp_tool_call(control, request),
            wire::McpMethod::Other => {
                return Ok(vec![mcp::error(
                    &control,
                    request.id.as_ref(),
                    codes::METHOD_NOT_FOUND,
                    "Unsupported MCP method.",
                )]);
            }
        };
        Ok(vec![mcp::reply(
            &control,
            request.id.as_ref(),
            mcp::Outcome::Success { result },
        )])
    }
    fn cancel_mcp(&mut self, params: Value) -> Result<()> {
        #[derive(Deserialize)]
        struct Cancel {
            #[serde(rename = "requestId")]
            id: Value,
        }
        if let Some(cancel) = crate::observability::optional(
            serde_json::from_value::<Cancel>(params),
            "claude_mcp_cancel",
        ) {
            let ids = self.callbacks.matching_requests(|_, mcp| *mcp == cancel.id);
            for id in ids {
                self.callbacks.requests.remove(&id);
                self.callbacks.cancelled.insert(id);
            }
            if self.callbacks.cancelled.len() > CANCELLED_CALLBACK_LIMIT {
                return Err(protocol());
            }
        }
        Ok(())
    }
    fn mcp_tool_call(&mut self, control: String, request: wire::McpRequest) -> Result<Vec<Value>> {
        if matches!(
            self.steering_permit,
            Some(super::super::SteeringPermit::Canary)
        ) {
            self.canary_tool()?;
            return Ok(vec![mcp::error(
                &control,
                request.id.as_ref(),
                codes::SERVER_ERROR,
                "Native steering canary permits only its fixed file Read.",
            )]);
        }
        if self.guard.sealed() {
            return Ok(vec![mcp::error(
                &control,
                request.id.as_ref(),
                codes::SERVER_ERROR,
                "Agent is shutting down.",
            )]);
        }
        let turn = self.active_id()?;
        let Some(call) = crate::observability::optional(
            serde_json::from_value::<wire::ToolCall>(request.params),
            "claude_tool_call",
        ) else {
            return Ok(vec![mcp::error(
                &control,
                request.id.as_ref(),
                codes::INVALID_PARAMS,
                "Invalid tool call arguments.",
            )]);
        };
        #[cfg(feature = "comb")]
        if call.name.starts_with("comb_")
            && self.callbacks.retired_comb_turn.as_deref() == Some(&turn)
        {
            return Ok(vec![mcp::error(
                &control,
                request.id.as_ref(),
                codes::SERVER_ERROR,
                "This turn's Comb calls have been cancelled.",
            )]);
        }
        let mcp_id = request.id.ok_or_else(protocol)?;
        if self.callbacks.requests.len() >= MCP_CALLBACK_LIMIT {
            return Err(protocol());
        }
        self.callbacks.next_id += 1;
        let id = self.callbacks.next_id;
        let native_id = crate::runtime::tool_registry::request_id(
            "claude",
            &turn,
            &api::value((&control, &mcp_id)),
        );
        self.callbacks.requests.insert(id, (control, mcp_id));
        #[cfg(feature = "comb")]
        if call.name.starts_with("comb_") {
            self.callbacks.comb_turns.insert(id, turn.clone());
        }
        self.frames.push_back(api::value(control::ToolCall {
            id,
            method: ServerMethod::ToolCall,
            params: control::ToolArguments {
                thread_id: &self.config.id,
                turn_id: &turn,
                call_id: native_id,
                tool: call.name,
                arguments: call.arguments,
            },
        }));
        Ok(vec![])
    }
    pub(super) fn tool_result(&mut self, frame: ToolResponse) -> Result<Vec<Value>> {
        let id = frame.id().as_u64().ok_or_else(protocol)?;
        #[cfg(feature = "comb")]
        self.callbacks.comb_turns.remove(&id);
        if self.callbacks.cancelled.remove(&id) {
            return Ok(vec![]);
        }
        let outcome = match frame {
            ToolResponse::Success { result, .. } => {
                let content = result
                    .content_items
                    .into_iter()
                    .map(|c| mcp::Text {
                        kind: "text",
                        text: c.text,
                    })
                    .collect();
                mcp::Outcome::Success {
                    result: mcp::ResultBody::Tool(mcp::ToolResult {
                        content,
                        is_error: !result.success,
                    }),
                }
            }
            ToolResponse::Failure { .. } => mcp::Outcome::Failure {
                error: mcp::Failure {
                    code: codes::INTERNAL_ERROR,
                    message: "Hive could not complete this tool request.",
                },
            },
        };
        let (control, mcp) = self.callbacks.requests.remove(&id).ok_or_else(protocol)?;
        Ok(vec![mcp::reply(&control, Some(&mcp), outcome)])
    }

    fn hook(&mut self, callback: wire::CallbackId, hook: wire::Hook) -> Result<Value> {
        self.check_session(&hook.session_id)?;
        if callback == wire::CallbackId::Guard
            && hook.event == wire::HookEvent::PreToolUse
            && self.guard.sealed()
        {
            return Ok(api::value(control::HookOutput {
                hook_specific_output: control::HookDecision {
                    hook_event_name: "PreToolUse",
                    permission_decision: "deny",
                    updated_input: None,
                    permission_decision_reason: "Agent is shutting down.",
                },
            }));
        }
        self.active_id()?;
        let tool_id = hook.tool_use_id.ok_or_else(protocol)?;
        match (callback, hook.event) {
            (wire::CallbackId::Guard, wire::HookEvent::PreToolUse) => {
                self.guard_tool(tool_id, hook.tool_name, hook.tool_input)
            }
            (
                wire::CallbackId::Finished,
                wire::HookEvent::PostToolUse | wire::HookEvent::PostToolUseFailure,
            ) => {
                self.tools.active.remove(&tool_id);
                // The agent's own Skill call settling ends its fork, so guidance
                // below reaches the agent; a fork's tools never receive it.
                self.settle_fork(&tool_id);
                let owned = self.tools.owners.remove(&tool_id).is_some_and(|owner| {
                    owner.allowed
                        && owner.assistant_seen
                        && self
                            .turn
                            .active
                            .as_ref()
                            .is_some_and(|t| t.id == owner.turn)
                });
                if let Some(permit) = self.tools.shell_permits.remove(&tool_id) {
                    self.guard.revoke(&permit.token);
                }
                // Steering guidance is for the agent, never a forked skill's subagent.
                if owned && hook.event == wire::HookEvent::PostToolUse && self.tools.fork.is_none()
                {
                    return Ok(self.hook_guidance().unwrap_or_else(mcp::empty));
                }
                Ok(mcp::empty())
            }
            _ => Err(protocol()),
        }
    }
    fn guard_tool(
        &mut self,
        tool_id: String,
        tool_name: Option<String>,
        tool_input: Value,
    ) -> Result<Value> {
        self.canary_tool()?;
        let name = tool_name.ok_or_else(protocol)?;
        let allowed = self
            .allowed(&name, &tool_input)
            .inspect_err(|error| {
                crate::observability::observation_failed("claude_hook_policy", error);
            })
            .unwrap_or(false);
        if !allowed {
            denied(&name, "hook");
        }
        let mut input = tool_input;
        if allowed {
            self.observe_tool(&tool_id, &name, true)?;
            if let Some(owner) = self.tools.owners.get_mut(&tool_id) {
                owner.allowed = true;
            }
            if self.tools.active.len() >= ACTIVE_TOOL_LIMIT
                || !self.tools.active.insert(tool_id.clone())
            {
                return Err(protocol());
            }
            if wire::NativeTool::parse(&name) == wire::NativeTool::Bash {
                // Replace only the approved shell command; preserve every other native input field.
                let command = input["command"].as_str().ok_or_else(protocol)?;
                // A read-only agent's git command runs only in its checked rendering.
                let command = if self.config.shell() == Shell::ReadOnlyGit {
                    read_only_git::parse(command).ok_or_else(protocol)?.render()
                } else {
                    command.to_owned()
                };
                let token = self.guard.expect()?;
                let command = format!("# {TOKEN_PREFIX}{token}\n{command}");
                self.tools.shell_permits.insert(
                    tool_id,
                    super::ShellPermit {
                        token,
                        command: command.clone(),
                        permission_used: false,
                    },
                );
                input["command"] = Value::String(command);
            }
        }
        Ok(api::value(control::HookOutput {
            hook_specific_output: control::HookDecision {
                hook_event_name: "PreToolUse",
                permission_decision: if allowed { "allow" } else { "deny" },
                updated_input: Some(input),
                permission_decision_reason: if !allowed
                    && wire::NativeTool::parse(&name) == wire::NativeTool::Bash
                    && self.config.shell() == Shell::ReadOnlyGit
                {
                    "Read-only agents may run one allowlisted read-only git command without \
                        shell syntax."
                } else {
                    "Hive agent permissions"
                },
            },
        }))
    }
    fn canary_tool(&mut self) -> Result<()> {
        if matches!(
            self.steering_permit,
            Some(super::super::SteeringPermit::Canary)
        ) {
            self.tools.canary_count += 1;
            if self.tools.canary_count > CANARY_TOOL_LIMIT {
                return Err(Error::output_limit(
                    "Native canary exceeded its tool-call limit.",
                ));
            }
        }
        Ok(())
    }
    fn allowed(&self, full_name: &str, input: &Value) -> Result<bool> {
        self.active_id()?;
        let name = wire::NativeTool::parse(full_name);
        if matches!(
            self.steering_permit,
            Some(super::super::SteeringPermit::Canary)
        ) {
            let args: PolicyInput = parse(input.clone())?;
            return Ok(name == wire::NativeTool::Read
                && args
                    .file_path
                    .as_deref()
                    .is_some_and(|path| Path::new(path) == self.config.cwd.join("fixed.txt")));
        }
        if let Some(tool) = full_name.strip_prefix("mcp__hive__") {
            return Ok(crate::runtime::tool_registry::advertised(
                self.config.tool_catalog,
                tool,
                self.config.comb_reviewer,
            ));
        }
        if self.config.mcp_servers.keys().any(|server| {
            full_name
                .strip_prefix(&format!("mcp__{server}__"))
                .is_some_and(|tool| !tool.is_empty())
        }) {
            return Ok(true);
        }
        // A skill only adds instructions; every tool it then calls is checked
        // here. A forked skill cannot start another skill: a second fork would
        // be a second task.
        if name == wire::NativeTool::Skill {
            return Ok(self.config.skills_enabled() && self.tools.fork.is_none());
        }
        let args: PolicyInput = parse(input.clone())?;
        match name {
            wire::NativeTool::Read | wire::NativeTool::Glob | wire::NativeTool::Grep => Ok(true),
            wire::NativeTool::Bash => Ok(!args.run_in_background
                && (!args.unsandboxed || self.config.shell() == Shell::Unrestricted)
                && args.command.is_some_and(|command| {
                    !command.contains(TOKEN_PREFIX)
                        && match self.config.shell() {
                            Shell::Full | Shell::Unrestricted => true,
                            Shell::ReadOnlyGit => read_only_git::parse(&command).is_some(),
                            Shell::None => false,
                        }
                })),
            wire::NativeTool::Edit | wire::NativeTool::Write if args.notebook_path.is_none() => {
                Ok(args
                    .file_path
                    .is_some_and(|path| self.config.writable(&path)))
            }
            wire::NativeTool::NotebookEdit if args.file_path.is_none() => Ok(args
                .notebook_path
                .is_some_and(|path| self.config.writable(&path))),
            _ => Ok(false),
        }
    }
    fn shell_permission(&mut self, id: Option<&str>, input: &Value) -> bool {
        if self.config.shell() == Shell::None {
            return false;
        }
        let Some(args) = crate::observability::optional(
            serde_json::from_value::<PolicyInput>(input.clone()),
            "claude_shell_policy",
        ) else {
            return false;
        };
        if args.run_in_background
            || (args.unsandboxed && self.config.shell() != Shell::Unrestricted)
        {
            return false;
        }
        let Some(command) = args.command else {
            return false;
        };
        let mut matching = self.tools.shell_permits.iter_mut().filter(|(key, permit)| {
            id.is_none_or(|id| *key == id)
                && !permit.permission_used
                && permit.command == command
                && self.guard.unregistered(&permit.token)
        });
        let Some((_, permit)) = matching.next() else {
            return false;
        };
        if matching.next().is_some() {
            return false;
        }
        permit.permission_used = true;
        true
    }
}
#[derive(Deserialize, Default)]
struct PolicyInput {
    command: Option<String>,
    file_path: Option<String>,
    notebook_path: Option<String>,
    #[serde(default)]
    run_in_background: bool,
    #[serde(default, rename = "dangerouslyDisableSandbox")]
    unsandboxed: bool,
}
