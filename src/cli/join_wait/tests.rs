use super::*;
use crate::{cli::response_tests::Stub, runtime::api::InputContext};
use serde_json::{Value, json};

fn rejected(field: &str) -> Value {
    api::value(api::ErrorResponse {
        error: InputContext::ReadJoin.error(
            hive_input::Code::InvalidValue,
            field,
            "a currently retained join",
        ),
    })
}

#[test]
fn join_wait_initial_rejection_names_the_join_flag_without_echoing_the_id() {
    let stub = Stub::new(vec![rejected("/target")]);
    let error = run(stub.root(), "private-canary", 3).err().unwrap();
    let value = serde_json::to_value(&error).unwrap();
    assert_eq!(value["code"], "invalid_value");
    assert_eq!(value["field"], "--join");
    assert!(!value.to_string().contains("private-canary"));
    assert!(
        value["examples"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str().unwrap().contains("wait --join"))
    );
    assert_eq!(stub.finish().len(), 1);
}

#[test]
fn join_wait_impossible_initial_input_pointers_are_protocol_failures() {
    for pointer in ["/after", "/limit", "/unknown"] {
        let stub = Stub::new(vec![rejected(pointer)]);
        assert_eq!(
            run(stub.root(), "joined", 3).err().unwrap().code(),
            "daemon_protocol_error"
        );
        assert_eq!(stub.finish().len(), 1);
    }
}

#[test]
fn join_wait_later_loss_and_operational_errors_keep_observation_outcomes() {
    let snapshot = json!({"join_id":"joined","requester":"human","request_id":"round",
        "created_at":1,"deadline":null,"state":"collecting","members":[]});
    let stub = Stub::new(vec![snapshot.clone(), rejected("/target")]);
    let outcome = run(stub.root(), "joined", 3).unwrap();
    assert!(!outcome.success && outcome.input.is_none());
    assert_eq!(outcome.value["wait"], "observation_unavailable");
    assert_eq!(outcome.value["join"], snapshot);
    assert_eq!(outcome.value["error"]["field"], "/target");
    assert_eq!(stub.finish().len(), 2);
    let failure = json!({"error":{"code":"database_error","message":"Cannot read join."}});
    let stub = Stub::new(vec![failure.clone()]);
    let outcome = run(stub.root(), "joined", 3).unwrap();
    assert!(!outcome.success && outcome.input.is_none());
    assert_eq!(outcome.value["wait"], "observation_unavailable");
    assert_eq!(outcome.value["error"], failure["error"]);
    assert!(outcome.value["join"].is_null());
    stub.finish();
}
