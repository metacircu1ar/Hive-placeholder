//! Validate provider frames and persist tool and turn outcomes.
use super::{
    ActiveTurn, AgentState, Control, Error, EventKind, ExecutionHandle, IncomingFrame, Input,
    MessageState, ParamsRef, RequestEnvelope, Result, ServerMethod, Session, Value, outgoing,
    response, validate_tool,
};
use crate::runtime::Core;
use crate::runtime::api::{self, events};
use crate::runtime::branching::FinishedTurn;
use crate::runtime::domain::AuthCutoff;
use crate::runtime::provider::protocol::ItemKind;
use crate::runtime::{
    hub::Caller,
    provider::{error::protocol, protocol::CoordinationTool},
};
use crate::state;

pub(super) async fn handle(
    execution: ExecutionHandle<'_>,
    rpc: &mut Session,
    thread_id: &str,
    active: &mut Option<ActiveTurn>,
    frame: IncomingFrame,
) -> Result<()> {
    let Some(method) = frame.method() else {
        return Ok(());
    };
    let params = frame.params();
    let matches = params.thread_id() == Some(thread_id)
        && active
            .as_ref()
            .is_some_and(|ActiveTurn { turn_id: turn, .. }| params.turn_id() == Some(turn));
    if frame.has_id() {
        handle_request(execution, rpc, &frame, method, matches).await
    } else if *method == ServerMethod::TurnCompleted && params.thread_id() == Some(thread_id) {
        let clean = finish_turn(execution, active, params).await?;
        if clean && rpc.codex() {
            // Persist the provider outcome first, but keep this worker/session
            // claimed until bounded quiet is observed. No input is replayed
            // if cleanup fails; execute() still closes through owned retirement.
            let turn = params.nested_turn_id().ok_or_else(protocol)?.to_owned();
            let observation = rpc.observe_terminal_cleanup(thread_id, &turn).await?;
            execution
                .agent(move |core, agent| {
                    core.store().event(
                        agent,
                        EventKind::TerminalCleanupObserved,
                        api::value(events::TerminalCleanupObserved {
                            turn_id: &turn,
                            scope: "conversation",
                            remaining: 0,
                            next_cursor: None,
                            observation,
                        }),
                    )
                })
                .await?;
        }
        Ok(())
    } else if matches {
        let call = active
            .as_ref()
            // Message senders are stored as exact identities, not live authorization contexts.
            .filter(|turn| turn.message.sender == Caller::Human.name() || turn.message.joined)
            .map(|turn| turn.message.id.clone());
        handle_notice(execution, method, params, call).await
    } else {
        Ok(())
    }
}
pub(super) async fn handle_request(
    execution: ExecutionHandle<'_>,
    rpc: &mut Session,
    frame: &IncomingFrame,
    method: &ServerMethod,
    matches: bool,
) -> Result<()> {
    let params = frame.params();
    let result = if *method == ServerMethod::ToolCall && matches {
        dispatch_tool(execution, params).await?
    } else {
        Err(Error::unsupported_provider_request(
            "Provider requested unsupported input or approval. This run uses \
                approvalPolicy never.",
        ))
    };
    let success = result.is_ok();
    if success
        && matches!(
            params.tool(),
            Some(CoordinationTool::Spawn | CoordinationTool::Batch)
        )
    {
        let placement = execution
            .agent(|core, agent| core.store().placement(&agent.id))
            .await?;
        rpc.activity(placement, true, false, false);
    }
    let value = response(result);
    rpc.send(outgoing::tool_reply(frame.raw_id(), &value, success))
        .await?;
    if *method != ServerMethod::ToolCall {
        return Err(Error::provider_input_required(
            "Provider requested input unsupported by this runtime increment.",
        ));
    }
    Ok(())
}
/// Outer errors abort before a tool reply; inner errors are serialized as tool results.
#[expect(
    clippy::expect_used,
    reason = "validate_tool accepts only object arguments and no later edit changes their \
        container."
)]
pub(super) async fn dispatch_tool(
    execution: ExecutionHandle<'_>,
    params: ParamsRef<'_>,
) -> Result<Result<Value>> {
    let tool_name = Input::<String>::from_value(params.raw_tool().clone());
    let name = tool_name.protocol_text()?;
    let op = name.strip_prefix("hive_").ok_or(Error::unsupported_tool(
        "Only Hive coordination tools are supported.",
    ))?;
    // Tool arguments remain opaque coordination input; only the existing op/ID edits follow.
    let mut args = params.arguments().clone();
    if let Err(error) = validate_tool(name, &args) {
        return Ok(Err(error));
    }
    Ok({
        args["op"] = api::value(op);
        let id = args["request_id"]
            .as_str()
            .unwrap_or(params.call_id().unwrap_or("unknown"))
            .to_owned();
        args.as_object_mut()
            .expect("validated tool arguments are an object")
            .remove("request_id");
        let args = RequestEnvelope::new(args);
        let turn = params.turn_id().map(str::to_owned);
        execution
            .shared
            .dispatch(
                Caller::Agent {
                    id: execution.id.to_owned(),
                    execution: execution.execution.to_owned(),
                    turn,
                },
                id,
                args,
            )
            .await
            .map_err(|error| api::tool_input_context(error, name))
    })
}

pub(super) async fn finish_turn(
    execution: ExecutionHandle<'_>,
    active: &mut Option<ActiveTurn>,
    params: ParamsRef<'_>,
) -> Result<bool> {
    let turn_id = params.nested_turn_id();
    if let Some(ActiveTurn {
        message,
        turn_id: turn,
        ..
    }) = active.take_if(|active| Some(active.turn_id.as_str()) == turn_id)
    {
        let status = params.status().unwrap_or("unknown").to_owned();
        // Keep the raw provider status in evidence, including unknown values.
        let provider_status = params.turn_status();
        let message_state = MessageState::from_provider_completion(provider_status);
        let failed = message_state == MessageState::Failed;
        let completed = CompletedTurn {
            message,
            status,
            provider_status,
            message_state,
            failed,
            turn,
        };
        let clean = execution
            .agent(move |core, agent| completed.record(core, agent))
            .await?;
        if failed {
            return Err(Error::turn_failed(
                "The provider reported a failed turn. Inspect events before retrying.",
            ));
        }
        return Ok(clean);
    }
    Ok(false)
}
struct CompletedTurn {
    message: super::Message,
    status: String,
    provider_status: Option<crate::runtime::domain::ProviderTurnStatus>,
    message_state: MessageState,
    failed: bool,
    turn: String,
}
impl CompletedTurn {
    #[expect(
        clippy::expect_used,
        reason = "The completion runs behind the live execution fence after startup bound \
            the native thread."
    )]
    fn record(&self, core: &mut Core, agent: &mut super::Agent) -> Result<bool> {
        // Read intent in the same writer transaction as completion:
        // redirect can arrive after the worker's last lifecycle snapshot.
        let clean = self.message_state == MessageState::Interrupted
            || agent.branch_hold > 0
            || agent.control.is_some()
            || agent.redirect_pending;
        agent.clear_turn();
        if agent.control == Some(Control::Interrupt) {
            agent.control = None;
        }
        agent.state = if self.failed {
            AgentState::Failed
        } else {
            AgentState::Idle
        };
        if self.message_state != MessageState::Completed {
            agent.completion_pending = None;
        }
        core.store()
            .message_state(&self.message, self.message_state)?;
        if let Some(outcome) = self.provider_status {
            agent.last_finished_turn = Some(FinishedTurn {
                thread: agent.thread_id.clone().expect("active native conversation"),
                execution: agent.execution_id.clone().expect("active execution"),
                turn: self.turn.clone(),
                message: Some(self.message.id.clone()),
                outcome,
                at: state::now(),
            });
        }
        if agent.branch_hold > 0 {
            core.store().deadlines_changed();
        }
        self.settle_controls(core, agent)?;
        core.store().save(agent)?;
        core.store().event(
            agent,
            EventKind::TurnFinished,
            api::value(events::TurnFinished {
                turn_id: Some(&self.turn),
                status: &self.status,
                message_id: &self.message.id,
            }),
        )?;
        if self.message_state == MessageState::Completed {
            core.confirm_login_turn(agent)?;
            core.finish_task_turn(agent, Some(&self.message.id))?;
        }
        core.release_auth_probe(agent, Some(&self.message.id))?;
        core.store().save(agent)?;
        Ok(clean)
    }
    #[expect(
        clippy::expect_used,
        reason = "The completion runs behind the live execution fence before clearing \
            execution identity."
    )]
    fn settle_controls(&self, core: &mut Core, agent: &mut super::Agent) -> Result<()> {
        if self.failed
            && agent.auth_cutoff.as_ref().map(AuthCutoff::turn) == Some(self.turn.as_str())
        {
            core.auth_cutoff(agent, &self.message.id, &self.turn)?;
        }
        agent.clear_cutoff();
        if agent.redirect_pending {
            core.store().finish_redirect(
                agent.execution_id.as_deref().expect("active execution"),
                &self.message.id,
            )?;
            agent.redirect_pending = false;
        }
        core.store().finish_steering(
            &agent.id,
            agent.execution_id.as_deref().expect("active execution"),
            &self.turn,
            self.message_state,
        )?;
        Ok(())
    }
}
pub(super) async fn handle_notice(
    execution: ExecutionHandle<'_>,
    method: &ServerMethod,
    params: ParamsRef<'_>,
    call: Option<String>,
) -> Result<()> {
    let method = method.clone();
    let params = params.to_owned();
    let rerouted = method == ServerMethod::ModelRerouted;
    execution
        .agent(move |core, agent| {
            let params = params.view();
            match method {
                ServerMethod::ItemCompleted => {
                    record_item(core, agent, params, call.as_deref())?;
                }
                ServerMethod::ModelRerouted => {
                    core.store()
                        .event(agent, EventKind::ModelRerouted, params.raw().clone())?;
                }
                ServerMethod::Error => core.store().event(
                    agent,
                    EventKind::ProviderError,
                    api::value(events::ProviderError {
                        turn_id: params.raw_turn_id(),
                        will_retry: params.will_retry(),
                    }),
                )?,
                _ => {}
            }
            Ok(())
        })
        .await?;
    if rerouted {
        return Err(Error::model_rerouted(
            "Provider rerouted the selected model; execution stopped.",
        ));
    }
    Ok(())
}

fn record_item(
    core: &mut Core,
    agent: &super::Agent,
    params: ParamsRef<'_>,
    call: Option<&str>,
) -> Result<()> {
    let item = params.item();
    match params.item_kind() {
        Some(ItemKind::AgentMessage) => {
            // Preserve absent versus null native annotations and the exact aggregate ID value.
            let data = api::value(events::AssistantOutput {
                turn_id: params.raw_turn_id(),
                item_id: ["id", "nativeMessageId"]
                    .into_iter()
                    .filter_map(|key| item.get(key))
                    .find(|value| value.is_string()),
                text: params.text(),
                phase: item.get("phase"),
                aggregate: item
                    .get("aggregation")
                    .map(|aggregation| events::OutputAggregation {
                        aggregation,
                        native_message_id: &item["nativeMessageId"],
                    }),
            });
            core.store()
                .assistant_output(agent, call, data, item["verifiedModel"] != false)?;
        }
        Some(ItemKind::CommandExecution | ItemKind::FileChange | ItemKind::DynamicToolCall) => {
            core.store().event(
                agent,
                EventKind::ToolCompleted,
                api::value(events::ToolCompleted {
                    turn_id: params.raw_turn_id(),
                    item,
                }),
            )?;
        }
        _ => {}
    }
    Ok(())
}
