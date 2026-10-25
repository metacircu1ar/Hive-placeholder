use super::*;
use crate::runtime::api::joins::JoinOptions;
use hive_input::Code;

fn rejected(raw: Value, code: &str, field: &str) {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    c.store().save(&agent("recipient")).unwrap();
    let error = call(&mut c, "human", "rejected", raw).unwrap_err();
    assert_eq!(error.code(), code);
    let wire = value(&error);
    assert_eq!(wire["field"], field);
    assert!(
        wire["examples"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
    assert!(wire.get("received").is_none());
    assert!(!wire.to_string().contains("PRIVATE"));
    assert!(!format!("{error:?}").contains("PRIVATE"));
    assert!(!c.store().request_used("human", "rejected").unwrap());
    assert!(c.store().next_message("recipient").unwrap().is_none());
    assert_eq!(
        c.store().agent("recipient").unwrap().task_outcome,
        TaskOutcome::Open
    );
}

#[test]
fn batch_fields_distinguish_missing_shape_count_and_header_errors() {
    rejected(json!({"op":"batch"}), "missing_argument", "/operations");
    for operations in [json!(null), json!({"PRIVATE":true}), json!("PRIVATE")] {
        rejected(
            json!({"op":"batch","operations":operations}),
            "invalid_json",
            "/operations",
        );
    }
    for operations in [json!([]), json!(vec![json!({}); 65])] {
        rejected(
            json!({"op":"batch","operations":operations}),
            "out_of_range",
            "/operations",
        );
    }
    for (item, code, field) in [
        (json!(null), "invalid_value", "/operations/0"),
        (json!([]), "invalid_value", "/operations/0"),
        (
            json!({"op":"send"}),
            "missing_argument",
            "/operations/0/request_id",
        ),
        (
            json!({"op":"PRIVATE","request_id":null}),
            "invalid_request_id",
            "/operations/0/request_id",
        ),
        (
            json!({"op":null,"request_id":"PRIVATE\n"}),
            "invalid_request_id",
            "/operations/0/request_id",
        ),
        (
            json!({"request_id":"valid"}),
            "missing_argument",
            "/operations/0/op",
        ),
        (
            json!({"op":{"send":null},"request_id":"valid"}),
            "invalid_value",
            "/operations/0/op",
        ),
        (
            json!({"op":"PRIVATE","request_id":"valid"}),
            "invalid_value",
            "/operations/0/op",
        ),
    ] {
        rejected(json!({"op":"batch","operations":[item]}), code, field);
    }
}

#[test]
fn batch_fingerprint_cap_counts_unknown_inner_keys_and_exact_utf8_bytes() {
    let cap = crate::runtime::limits::BATCH_INPUT_BYTES;
    let mut raw = json!({"op":"batch","operations":[{"op":"send","request_id":"one","extra":""}]});
    let overhead = raw.to_string().len();
    raw["operations"][0]["extra"] = json!("x".repeat(cap - overhead - 2) + "é");
    let envelope = RequestEnvelope::new(raw.clone());
    assert_eq!(envelope.fingerprint().len(), cap);
    assert_eq!(envelope.checked_batch_items().unwrap().len(), 1);
    assert!(envelope.fingerprint().contains("extra"));
    let text = raw["operations"][0]["extra"].as_str().unwrap().to_owned() + "x";
    raw["operations"][0]["extra"] = json!(text);
    let error = RequestEnvelope::new(raw).checked_batch_items().unwrap_err();
    assert_eq!(error.code(), "out_of_range");
    assert_eq!(value(error)["field"], "/operations");
    let operations = vec![json!({"op":"send","request_id":"id"}); 64];
    assert_eq!(
        RequestEnvelope::new(json!({"op":"batch","operations":operations}))
            .checked_batch_items()
            .unwrap()
            .len(),
        64
    );
}

#[test]
fn join_deadline_uses_checked_timestamp_and_preserves_absence() {
    let now = 1234;
    for seconds in [1, i64::MAX as u64 - now] {
        let options = JoinOptions {
            deadline_seconds: seconds.into(),
        };
        assert_eq!(options.deadline(now).unwrap(), Some(now + seconds));
    }
    assert_eq!(JoinOptions::default().deadline(now).unwrap(), None);
    for raw in [
        json!(0),
        json!(null),
        json!("PRIVATE"),
        json!(u64::MAX),
        json!(i64::MAX as u64 - now + 1),
    ] {
        let options = JoinOptions {
            deadline_seconds: Input::from_value(raw),
        };
        let error = options.deadline(now).unwrap_err();
        assert_eq!(value(&error)["field"], "/join/deadline_seconds");
        assert!(!format!("{error:?}").contains("PRIVATE"));
    }
    assert!(
        JoinOptions {
            deadline_seconds: 1.into()
        }
        .deadline(u64::MAX)
        .is_err()
    );
}

#[test]
fn joined_batch_rejections_keep_fields_and_roll_back_termination() {
    let send = json!({"op":"send","target":"recipient","body":"work","request_id":"one"});
    for options in [json!(null), json!([]), json!({"PRIVATE":1})] {
        rejected(
            json!({"op":"batch","join":options,"operations":[send]}),
            "invalid_json",
            "/join",
        );
    }
    let mut invalid_mode = send.clone();
    invalid_mode["mode"] = json!("steer");
    rejected(
        json!({"op":"batch","join":{},"operations":[invalid_mode]}),
        "conflicting_arguments",
        "/operations/0/mode",
    );
    rejected(
        json!({"op":"batch","join":{},"operations":[{"op":"terminate","target":"recipient","request_id":"end"}]}),
        "conflicting_arguments",
        "/operations",
    );
}

#[test]
fn indexed_errors_preserve_operational_identity_and_input_cause() {
    let operational = Error::unknown_agent("No agent has this ID.");
    assert_eq!(batch::at_index(operational, 3).code(), "unknown_agent");
    let error = batch::error(Code::InvalidValue, "/target", "an agent target")
        .with_source(std::io::Error::from_raw_os_error(libc::EIO));
    let error = batch::at_index(error, 3);
    assert_eq!(value(&error)["field"], "/operations/3/target");
    assert_eq!(error.io_raw_os_error(), Some(libc::EIO));
}
