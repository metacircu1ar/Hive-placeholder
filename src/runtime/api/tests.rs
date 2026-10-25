use super::*;
mod approvals;
mod batch_inputs;
#[cfg(feature = "comb")]
mod collector_inputs;
mod context_inputs;
mod control_inputs;
mod gc;
mod joins;
mod lifetimes;
mod list_filters;
mod observation_inputs;
mod options;
mod pagination;
mod purge;
mod redirect;
mod relative_targets;
mod scheduled;
mod selection_inputs;
mod steering;
use serde_json::Value;

use crate::{
    config::{Profile, Role},
    error::Error,
    runtime::{
        Core,
        domain::{Agent, AgentState, TaskOutcome},
        store::Store,
    },
};
use serde_json::json;
use std::path::{Path, PathBuf};

fn core(path: &Path) -> Core {
    crate::runtime::test_support::CoreBuilder::new(path).build()
}
fn agent(id: &str) -> Agent {
    crate::runtime::test_support::AgentBuilder::new(id)
        .configure(|agent| {
            agent.state = AgentState::Idle;
            agent.instructions = "original instructions".into();
        })
        .build()
}
fn call(core: &mut Core, caller: &str, id: &str, raw: Value) -> crate::error::Result<Value> {
    let caller = if caller == "human" {
        crate::runtime::hub::Caller::Human
    } else {
        crate::runtime::hub::Caller::test_agent(caller)
    };
    let args = RequestEnvelope::new(raw);
    let mut facts =
        crate::runtime::core::preflight::RequestInput::capture(core, &caller, &args).resolve();
    for spawn in facts.spawns.values_mut() {
        for member in &mut spawn.members {
            member.catalog = Some(Ok(vec![crate::runtime::provider::protocol::Model::parse(
                &json!({"model":"mock","supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"high"}]}),
            )]));
        }
    }
    core.dispatch(&caller, id, &args, &facts)
}
fn assert_error(result: crate::error::Result<Value>, expected: Error) {
    assert_eq!(
        serde_json::to_value(result.unwrap_err()).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}
fn invalid() -> Error {
    Error::invalid_request("Invalid operation or arguments.")
}
fn completed() -> Error {
    Error::agent_completed("Completed, cancelled or lifetime-expired agents cannot accept work.")
}

fn assert_batch_rollback(core: &mut Core) {
    let send = json!({"op":"send","target":"recipient","body":"work","request_id":"inner"});
    assert_error(
        call(
            core,
            "caller",
            "batch",
            json!({"op":"batch","operations":[send, {"op":"terminate"}]}),
        ),
        batch::at_index(
            batch::error(
                hive_input::Code::MissingArgument,
                "/request_id",
                "a request_id string of 1 to 128 UTF-8 bytes without control characters",
            ),
            1,
        ),
    );
    assert!(
        core.store().messages("recipient", None, 200).unwrap()["messages"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        core.store()
            .prior(
                "caller",
                "inner",
                &json!({"op":"send","target":"recipient","body":"work"}).to_string()
            )
            .unwrap()
            .is_none()
    );
    assert_error(
        call(
            core,
            "caller",
            "batch",
            json!({
                "op": "batch",
                "operations": [
                    {
                        "op": "send",
                        "target": "absent",
                        "body": "work",
                        "request_id": "first"
                    },
                    null
                ]
            }),
        ),
        Error::unknown_agent("No agent has this ID."),
    );
    assert_error(
        call(
            core,
            "caller",
            "batch",
            json!({
                "op": "batch",
                "operations": [
                    {
                        "op": "terminate",
                        "target": "caller",
                        "request_id": "cancel"
                    },
                    {
                        "op": "send",
                        "request_id": "after-cancel"
                    }
                ]
            }),
        ),
        completed(),
    );
    assert_eq!(
        core.store().agent("caller").unwrap().task_outcome,
        TaskOutcome::Open
    );
}

fn assert_batch_operation_shape(core: &mut Core) {
    // Serde unit enums also accept {"send": null}; the wire op must still
    // be a string, rejected before nested replay or caller eligibility.
    assert_error(
        call(
            core,
            "caller",
            "object-op",
            json!({
                "op": "batch",
                "operations": [
                    {
                        "op": "terminate",
                        "target": "caller",
                        "request_id": "cancel"
                    },
                    {
                        "op": {
                            "send": null
                        },
                        "request_id": "object"
                    }
                ]
            }),
        ),
        batch::at_index(
            batch::error(
                hive_input::Code::InvalidValue,
                "/op",
                "spawn, send or terminate",
            ),
            1,
        ),
    );
    assert_eq!(
        core.store().agent("caller").unwrap().task_outcome,
        TaskOutcome::Open
    );
    let accepted = call(
        core,
        "caller",
        "after-rollback",
        json!({"op":"send","target":"recipient","body":"work"}),
    )
    .unwrap();
    assert_eq!(accepted["delivery_state"], "accepted");
    assert_error(
        call(
            core,
            "caller",
            "object-replay",
            json!({
                "op": "batch",
                "operations": [
                    {
                        "op": {
                            "send": null
                        },
                        "request_id": "after-rollback"
                    }
                ]
            }),
        ),
        batch::at_index(
            batch::error(
                hive_input::Code::InvalidValue,
                "/op",
                "spawn, send or terminate",
            ),
            0,
        ),
    );
    assert_eq!(
        core.store().agent("caller").unwrap().state,
        AgentState::Idle
    );
}

mod cases_behaviour_1;
mod cases_lifecycle_1;
mod cases_presentation_1;
mod cases_requests_1;
mod cases_streaming_1;
mod cases_transport_1;
