//! Behaviour regressions.
use super::*;

#[test]
fn boundary_inputs_preserve_json_type_leniency_and_byte_limits() {
    for raw in [
        Value::Null,
        json!(false),
        json!(true),
        json!(-1),
        json!(1.0),
        json!("1"),
        json!([]),
        json!({}),
    ] {
        assert_eq!(
            Input::<u64>::from_value(raw.clone()).copied(),
            raw.as_u64(),
            "{raw}"
        );
        assert_eq!(
            Input::<bool>::from_value(raw.clone()).copied() == Some(true),
            raw == true,
            "{raw}"
        );
        assert_eq!(
            Input::<String>::from_value(raw.clone()).as_str(),
            raw.as_str(),
            "{raw}"
        );
    }
    for number in [0, 1, i64::MAX as u64, u64::MAX] {
        assert_eq!(
            Input::<u64>::from_value(json!(number)).copied(),
            Some(number)
        );
    }
    for text in [" ".to_owned(), "x".repeat(65536), "é".repeat(32768)] {
        assert_eq!(
            Input::Valid(text.clone())
                .required(InputContext::Send, "/body")
                .unwrap(),
            text
        );
    }
    for text in [String::new(), "x".repeat(65537), "é".repeat(32769)] {
        assert!(matches!(
            Input::Valid(text).required(InputContext::Send, "/body"),
            Err(Error::Input { .. })
        ));
    }
}

#[test]
fn malformed_and_unknown_operations_remain_deferred() {
    let temp = crate::runtime::test_directory();
    let mut core = core(temp.path());
    for raw in [
        Value::Null,
        json!([]),
        json!({}),
        json!({"op":false}),
        json!({"op":""}),
        json!({"op":"x".repeat(65537)}),
    ] {
        assert!(matches!(
            RequestEnvelope::new(raw.clone()).request,
            Request::Invalid
        ));
        assert_error(call(&mut core, "human", "invalid", raw), invalid());
    }
    for op in ["unknown", "invalid", "unsupported", " ", &"x".repeat(65536)] {
        assert!(matches!(
            RequestEnvelope::new(json!({"op":op})).request,
            Request::Unsupported
        ));
        assert_error(
            call(&mut core, "human", "unknown", json!({"op":op})),
            Error::unsupported_operation("Operation is not implemented in this runtime increment."),
        );
    }
    let mut terminal = agent("terminal");
    terminal.task_outcome = TaskOutcome::Completed;
    core.store().save(&terminal).unwrap();
    assert_error(
        call(&mut core, "terminal", "invalid", json!({})),
        completed(),
    );
    assert_error(
        call(&mut core, "terminal", "unknown", json!({"op":"unknown"})),
        completed(),
    );
    assert!(call(&mut core, "terminal", "read-only", json!({"op":"agents"})).is_ok());
}

#[test]
#[cfg(feature = "comb")]
fn explicit_evidence_replays_and_collector_administration_is_human_only() {
    let temp = crate::runtime::test_directory();
    let mut core = core(temp.path());
    let request =
        json!({"op":"evidence", "kind":"correction", "summary":"Explicitly public correction."});
    let first = call(&mut core, "human", "evidence-1", request.clone()).unwrap();
    assert_eq!(
        first,
        call(&mut core, "human", "evidence-1", request).unwrap()
    );
    assert_eq!(
        call(
            &mut core,
            "human",
            "evidence-1",
            json!({"op":"evidence","kind":"correction","summary":"Different"})
        )
        .unwrap_err()
        .code(),
        "request_conflict"
    );
    for value in [
        json!({"op":"collectors"}),
        json!({"op":"collector_admin","action":"discard","destination":"shared"}),
    ] {
        let request = RequestEnvelope::new(value);
        assert!(
            request
                .request
                .caller_policy(&crate::runtime::hub::Caller::test_agent("agent"))
                .is_err()
        );
    }
}
