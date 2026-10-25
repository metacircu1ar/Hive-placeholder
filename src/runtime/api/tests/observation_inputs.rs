use super::control_inputs::assert_input;
use super::*;

#[test]
fn stalled_reads_report_field_failures_without_echoing_cursor_contents() {
    let temp = crate::runtime::test_directory();
    let c = core(temp.path());
    let cases = [
        (
            json!({"stalled_after":{"idle_since":1,"root_id":"a"}}),
            "missing_argument",
            "/stalled_seconds",
        ),
        (
            json!({"stalled_seconds":null}),
            "invalid_value",
            "/stalled_seconds",
        ),
        (
            json!({"stalled_seconds":"private-canary"}),
            "invalid_value",
            "/stalled_seconds",
        ),
        (
            json!({"stalled_seconds":0}),
            "out_of_range",
            "/stalled_seconds",
        ),
        (
            json!({"stalled_seconds":u64::MAX}),
            "out_of_range",
            "/stalled_seconds",
        ),
        (
            json!({"stalled_seconds":1,"after":0}),
            "conflicting_arguments",
            "/after",
        ),
        (
            json!({"stalled_seconds":1,"parent":null}),
            "conflicting_arguments",
            "/parent",
        ),
        (
            json!({"stalled_seconds":1,"auth":"private-canary"}),
            "conflicting_arguments",
            "/auth",
        ),
        (
            json!({"stalled_seconds":1,"stalled_after":null}),
            "invalid_value",
            "/stalled_after",
        ),
        (
            json!({"stalled_seconds":1,"stalled_after":[1,"private-canary"]}),
            "invalid_value",
            "/stalled_after",
        ),
        (
            json!({"stalled_seconds":1,"stalled_after":{"idle_since":1,"root_id":"a","extra":true}}),
            "invalid_value",
            "/stalled_after",
        ),
        (
            json!({"stalled_seconds":1,"stalled_after":{"idle_since":1,"root_id":""}}),
            "out_of_range",
            "/stalled_after/root_id",
        ),
        (
            json!({"stalled_seconds":1,"stalled_after":{"idle_since":1,"root_id":"é".repeat(65)}}),
            "out_of_range",
            "/stalled_after/root_id",
        ),
        (
            json!({"stalled_seconds":1,"stalled_after":{"idle_since":u64::MAX,"root_id":"private-canary"}}),
            "out_of_range",
            "/stalled_after/idle_since",
        ),
    ];
    for (mut raw, code, field) in cases {
        raw["op"] = json!("agents");
        assert_input(pagination::observe(&c, raw), code, field, "agents");
    }
}

#[test]
fn stalled_reads_preserve_pagination_priority_and_valid_cursor_bounds() {
    let temp = crate::runtime::test_directory();
    let c = core(temp.path());
    // Outer read dispatch keeps pagination priority over stalled-field validation.
    assert_input(
        pagination::observe(&c, json!({"op":"agents","limit":0,"stalled_seconds":0})),
        "out_of_range",
        "/limit",
        "agents",
    );
    for id in ["é".repeat(64), " private\nroot ".into()] {
        assert!(
            pagination::observe(
                &c,
                json!({"op":"agents","stalled_seconds":1,
            "stalled_after":{"idle_since":i64::MAX,"root_id":id}})
            )
            .is_ok()
        );
    }
}

#[test]
fn cursor_decoders_reject_arrays_and_duplicate_fields_but_roundtrip_objects() {
    for (raw, gc) in [
        (r#"[1,"a"]"#, true),
        (r#"{"ended_at":1,"id":"a","id":"b"}"#, true),
        (r#"[1,"a"]"#, false),
        (r#"{"idle_since":1,"root_id":"a","root_id":"b"}"#, false),
    ] {
        if gc {
            assert!(serde_json::from_str::<GcCursor>(raw).is_err());
        } else {
            assert!(serde_json::from_str::<StalledCursor>(raw).is_err());
        }
    }
    let gc = GcCursor {
        ended_at: 0,
        id: "a".into(),
    };
    assert_eq!(
        serde_json::from_value::<GcCursor>(serde_json::to_value(&gc).unwrap()).unwrap(),
        gc
    );
    let stalled = StalledCursor {
        idle_since: 0,
        root_id: "a".into(),
    };
    assert_eq!(
        serde_json::from_value::<StalledCursor>(serde_json::to_value(&stalled).unwrap()).unwrap(),
        stalled
    );
}
