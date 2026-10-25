//! Literal evidence goldens distinguish absent fields from explicit native nulls.
use super::*;
use crate::runtime::api;
use serde_json::json;

fn exact(value: impl Serialize, expected: Value) {
    let value = api::value(value);
    assert_eq!(value, expected);
    assert_eq!(value.to_string(), expected.to_string());
}
#[test]
fn assistant_annotations_keep_presence_and_opaque_native_values() {
    let turn = json!({"opaque":[null]});
    let text = json!(["native", 17]);
    let native_id = json!({"id":false});
    for phase in [
        None,
        Some(Value::Null),
        Some(json!(false)),
        Some(json!({"raw":[]})),
    ] {
        for (aggregate, item_id) in [
            (None, None),
            (Some(Value::Null), Some(json!("msg_1"))),
            (Some(json!({"future":true})), None),
        ] {
            let mut expected = json!({"turn_id":turn,"text":text});
            if let Some(value) = &item_id {
                expected["item_id"] = value.clone();
            }
            if let Some(value) = &phase {
                expected["phase"] = value.clone();
            }
            if let Some(value) = &aggregate {
                expected["aggregation"] = value.clone();
                expected["native_message_id"] = native_id.clone();
            }
            exact(
                AssistantOutput {
                    turn_id: &turn,
                    item_id: item_id.as_ref(),
                    text: &text,
                    phase: phase.as_ref(),
                    aggregate: aggregate.as_ref().map(|aggregation| OutputAggregation {
                        aggregation,
                        native_message_id: &native_id,
                    }),
                },
                expected,
            );
        }
    }
}
#[test]
fn readiness_includes_steering_reason_only_alongside_proof_error() {
    for reason in [None, Some("missing_proof")] {
        for failed in [false, true] {
            let model = json!("model");
            let effort = json!("low");
            let sandbox = json!({"native":[null]});
            let mut expected = json!({"thread_id":"thread","model":model,"effort":effort,
                "options_revision":3,"selection_source":"session","sandbox":sandbox,"resumed":false});
            if failed {
                expected["steering_proof_error_code"] = json!("provider_unavailable");
                expected["steering_disabled_reason"] = json!(reason);
            }
            exact(
                SessionReady {
                    thread_id: "thread",
                    model: &model,
                    effort: &effort,
                    options_revision: 3,
                    selection_source: "session",
                    sandbox: &sandbox,
                    resumed: false,
                    steering: failed.then_some(SteeringDisabled {
                        steering_proof_error_code: "provider_unavailable",
                        steering_disabled_reason: reason,
                    }),
                },
                expected,
            );
        }
    }
}
#[test]
fn shared_process_and_binding_payloads_keep_exact_fields() {
    exact(
        ProviderStarted {
            provider_id: "provider",
            pgid: 17,
            process: ProcessStart {
                pid: 17,
                spawn_before: 1,
                spawn_after: 2,
            },
        },
        json!({"provider_id":"provider","pid":17,"pgid":17,"spawn_before":1,"spawn_after":2}),
    );
    exact(
        ThreadBound {
            thread_id: "thread",
            late_ack: None,
        },
        json!({"thread_id":"thread"}),
    );
    for late in [false, true] {
        exact(
            ThreadBound {
                thread_id: "thread",
                late_ack: Some(late),
            },
            json!({"thread_id":"thread","late_ack":late}),
        );
    }
    exact(
        TurnFinished {
            turn_id: None,
            status: "future-status",
            message_id: "message",
        },
        json!({"turn_id":null,"status":"future-status","message_id":"message"}),
    );
}
