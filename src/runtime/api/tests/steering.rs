use super::*;
use crate::runtime::domain::Control;

fn running(id: &str) -> Agent {
    let mut a = agent(id);
    a.state = AgentState::Running;
    a.execution_id = Some("execution".into());
    a.turn_id = Some("turn".into());
    a.steering = crate::runtime::domain::SteeringCapability::Supported;
    a
}

#[test]
fn tool_mode_enum_covers_sends_and_batches_without_expanding_other_operations() {
    for mode in ["next-turn", "steer", "redirect"] {
        let args = json!({"target":"a","body":"b","mode":mode,"request_id":"id"});
        validate_tool("hive_send", &args).unwrap();
        let mut item = args.clone();
        item["op"] = json!("send");
        validate_tool(
            "hive_batch",
            &json!({"request_id":"batch","operations":[item]}),
        )
        .unwrap();
        for op in ["spawn", "terminate"] {
            let mut item = if op == "spawn" {
                json!({"role":"r","body":"b","request_id":"id"})
            } else {
                json!({"target":"a","request_id":"id"})
            };
            item["mode"] = json!(mode);
            assert!(validate_tool(&format!("hive_{op}"), &item).is_err());
            item["op"] = json!(op);
            assert!(
                validate_tool(
                    "hive_batch",
                    &json!({"request_id":"batch","operations":[item]})
                )
                .is_err()
            );
        }
    }
    assert!(
        validate_tool(
            "hive_send",
            &json!({"target":"a","body":"b","mode":"invalid","request_id":"id"})
        )
        .is_err()
    );
}

#[test]
fn steering_acceptance_checks_lifecycle_and_reserves_only_accepted_ids() {
    let tmp = crate::runtime::test_directory();
    let mut c = core(tmp.path());
    let args = json!({"op":"send","target":"recipient","body":"guidance","mode":"steer"});
    c.store().save(&running("recipient")).unwrap();
    for condition in [
        "paused",
        "interrupt",
        "stop",
        "completed",
        "cancelled",
        "unknown",
    ] {
        c.store().begin().unwrap();
        let mut a = running("recipient");
        match condition {
            "paused" => a.paused = true,
            "interrupt" => a.control = Some(Control::Interrupt),
            "stop" => a.control = Some(Control::Stop),
            "completed" => a.task_outcome = TaskOutcome::Completed,
            "cancelled" => a.task_outcome = TaskOutcome::Cancelled,
            _ => {}
        }
        c.store().save(&a).unwrap();
        let mut request = args.clone();
        if condition == "unknown" {
            request["target"] = json!("unknown");
        }
        let expected = match condition {
            "completed" | "cancelled" => "agent_completed",
            "unknown" => "unknown_agent",
            _ => "no_active_turn",
        };
        assert_eq!(
            call(&mut c, "human", condition, request.clone())
                .unwrap_err()
                .code(),
            expected
        );
        assert!(
            c.store()
                .prior(
                    "human",
                    condition,
                    &RequestEnvelope::new(request).fingerprint()
                )
                .unwrap()
                .is_none()
        );
        c.store().rollback().unwrap();
    }
    assert!(
        c.store().messages("recipient", None, 200).unwrap()["messages"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    c.store().save(&running("recipient")).unwrap();
    let accepted = call(&mut c, "human", "paused", args.clone()).unwrap();
    assert_eq!(accepted["steering_target"]["turn_id"], "turn");
    for (key, value) in [("body", "different"), ("target", "other")] {
        let mut changed = args.clone();
        changed[key] = json!(value);
        assert_eq!(
            call(&mut c, "human", "paused", changed).unwrap_err().code(),
            "request_conflict"
        );
    }
    // Capability Ping is covered at the control reader in tests/runtime/steering.rs.
}

#[test]
fn failed_batch_rolls_back_steering_message_delivery_and_replay() {
    let tmp = crate::runtime::test_directory();
    let mut c = core(tmp.path());
    c.store().save(&running("recipient")).unwrap();
    let send = json!({"op":"send","target":"recipient","body":"guidance","mode":"steer"});
    let mut item = send.clone();
    item["request_id"] = json!("inner");
    let batch = json!({"op":"batch","operations":[item,{"op":"terminate","target":"missing","request_id":"fail"}]});
    assert!(call(&mut c, "human", "batch", batch).is_err());
    assert!(
        c.store().messages("recipient", None, 200).unwrap()["messages"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        c.store()
            .prior("human", "inner", &RequestEnvelope::new(send).fingerprint())
            .unwrap()
            .is_none()
    );
    let db = rusqlite::Connection::open(tmp.path().join("runtime.sqlite")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM deliveries", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn typed_delivery_mode_keeps_deferred_errors_and_optional_field_semantics() {
    let tmp = crate::runtime::test_directory();
    let mut c = core(tmp.path());
    for mode in [json!("future"), json!(""), json!("NEXT-TURN")] {
        assert_error(
            call(
                &mut c,
                "human",
                "unknown-mode",
                json!({"op":"send", "mode":mode}),
            ),
            Error::unsupported_operation("Delivery mode must be next-turn, steer, or redirect."),
        );
    }
    for mode in [
        None,
        Some(json!("next-turn")),
        Some(json!("steer")),
        Some(json!("redirect")),
    ] {
        let mut raw = json!({"op":"send","target":"missing", "body":"body"});
        if let Some(mode) = mode {
            raw["mode"] = mode;
        }
        assert_error(
            call(&mut c, "human", "absent-agent", raw),
            Error::unknown_agent("No agent has this ID."),
        );
    }
    for mode in [
        json!(null),
        json!(7),
        json!({}),
        json!("future"),
        json!("steer"),
        json!("redirect"),
    ] {
        assert_error(
            call(
                &mut c,
                "human",
                "delay-mode",
                json!({"op":"send","mode":mode,"delay_seconds":1}),
            ),
            Error::unsupported_operation("Delayed messages require next-turn mode."),
        );
        assert_error(
            call(
                &mut c,
                "human",
                "invalid-delay",
                json!({"op":"send","mode":mode,"delay_seconds":0}),
            ),
            Error::invalid_request(
                "delay_seconds must be an integer from 1 to 604800 seconds (7 days).",
            ),
        );
    }
}
