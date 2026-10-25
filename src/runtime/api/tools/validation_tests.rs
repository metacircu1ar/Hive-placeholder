use super::*;
use serde_json::json;

fn diagnostic(name: &str, args: &Value, code: &str, field: &str) -> Value {
    let error = validate_tool(name, args).unwrap_err();
    let wire = serde_json::to_value(error).unwrap();
    assert_eq!(wire["code"], code, "{name} {args}");
    assert_eq!(wire["field"], field, "{name} {args}");
    assert!(!wire["expected"].as_str().unwrap().is_empty());
    let example: Value = serde_json::from_str(wire["examples"][0].as_str().unwrap()).unwrap();
    assert_eq!(example["name"], name);
    validate_tool(name, &example["arguments"]).unwrap();
    assert!(wire.get("received").is_none());
    wire
}

#[test]
fn known_fields_get_specific_private_safe_tool_diagnostics() {
    for (name, args, code, field) in [
        (
            "hive_read",
            json!({"target":"a","limit":-1}),
            "invalid_value",
            "/limit",
        ),
        (
            "hive_read",
            json!({"target":"a","limit":1.0}),
            "invalid_value",
            "/limit",
        ),
        (
            "hive_read",
            json!({"target":"a","limit":null}),
            "invalid_value",
            "/limit",
        ),
        ("hive_status", json!({}), "missing_argument", "/target"),
        (
            "hive_send",
            json!({"target":"a","body":"secret","request_id":"r","mode":42}),
            "invalid_value",
            "/mode",
        ),
        (
            "hive_spawn",
            json!({"body":"secret","request_id":"r","auth":false}),
            "invalid_value",
            "/auth",
        ),
        (
            "hive_send",
            json!({"target":"a","body":"secret","request_id":"secret\n"}),
            "invalid_request_id",
            "/request_id",
        ),
        (
            "hive_send",
            json!({"target":"a","body":"secret","request_id":"r","mode":"secret"}),
            "invalid_value",
            "/mode",
        ),
        (
            "hive_status",
            json!({"target":"a","secret~/\n":true}),
            "unknown_argument",
            "/",
        ),
        (
            "hive_complete",
            json!({"target":"a","body":"secret","request_id":"r"}),
            "unknown_argument",
            "/",
        ),
        ("hive_agents", Value::Null, "invalid_value", "/"),
    ] {
        let error = diagnostic(name, &args, code, field);
        assert!(!error.to_string().contains("secret"));
    }
    let wire = serde_json::to_value(validate_tool("secret", &Value::Null).unwrap_err()).unwrap();
    assert_eq!(wire["field"], "/name");
    assert_eq!(wire["code"], "unknown_argument");
    assert!(!wire.to_string().contains("secret"));
    diagnostic(
        "hive_status",
        &json!({"target":"a", "x".repeat(300):true}),
        "unknown_argument",
        "/",
    );
}

#[test]
fn batch_fields_preserve_indices_and_private_decoder_causes() {
    for (operation, code, field) in [
        (json!([]), "invalid_value", "/operations/1"),
        (json!({}), "missing_argument", "/operations/1/op"),
        (json!({"op":"secret"}), "invalid_value", "/operations/1/op"),
        (json!({"op":17}), "invalid_value", "/operations/1/op"),
        (
            json!({"op":"send","request_id":"inner","body":"secret"}),
            "missing_argument",
            "/operations/1/target",
        ),
        (
            json!({"op":"terminate","request_id":"inner","target":"a","body":"secret"}),
            "unknown_argument",
            "/operations/1",
        ),
    ] {
        let args = json!({"request_id":"outer","operations":[{"op":"terminate","request_id":"first","target":"a"},operation]});
        let wire = diagnostic("hive_batch", &args, code, field);
        assert!(!wire.to_string().contains("secret"));
        if field.ends_with("/op") {
            assert!(
                std::error::Error::source(&validate_tool("hive_batch", &args).unwrap_err())
                    .is_some_and(<dyn std::error::Error>::is::<serde_json::Error>)
            );
        }
    }
    for operations in [json!([]), json!(vec![json!({}); 65])] {
        diagnostic(
            "hive_batch",
            &json!({"request_id":"r","operations":operations}),
            "out_of_range",
            "/operations",
        );
    }
}

#[test]
fn nested_schema_errors_use_only_known_fields() {
    for (name, args, code, field) in [
        (
            "hive_batch",
            json!({"request_id":"r","operations":[],"join":{"deadline_seconds":0}}),
            "invalid_duration",
            "/join/deadline_seconds",
        ),
        (
            "hive_batch",
            json!({"request_id":"r","operations":[],"join":{"deadline_seconds":u64::MAX}}),
            "out_of_range",
            "/join/deadline_seconds",
        ),
        (
            "hive_batch",
            json!({"request_id":"r","operations":[],"join":{"secret":0}}),
            "unknown_argument",
            "/join",
        ),
        (
            "hive_agents",
            json!({"stalled_after":{"root_id":"a"}}),
            "missing_argument",
            "/stalled_after/idle_since",
        ),
        (
            "hive_agents",
            json!({"stalled_after":{"idle_since":0,"root_id":"a","secret":0}}),
            "unknown_argument",
            "/stalled_after",
        ),
        (
            "hive_agents",
            json!({"stalled_after":{"idle_since":u64::MAX,"root_id":"a"}}),
            "out_of_range",
            "/stalled_after/idle_since",
        ),
        (
            "hive_agents",
            json!({"stalled_after":{"idle_since":0,"root_id":"a".repeat(129)}}),
            "invalid_value",
            "/stalled_after/root_id",
        ),
    ] {
        assert!(
            !diagnostic(name, &args, code, field)
                .to_string()
                .contains("secret")
        );
    }
}

#[test]
fn hosted_attachment_arrays_are_admitted_without_opening_sources() {
    for attachments in [
        json!([]),
        json!(["/nonexistent/review.txt"]),
        json!(vec!["/nonexistent/a"; 10]),
    ] {
        for (tool, mut args) in [
            ("hive_spawn", json!({"request_id":"spawn","body":"Review"})),
            (
                "hive_send",
                json!({"request_id":"send","target":"a","body":"Review"}),
            ),
        ] {
            args["attachments"] = attachments.clone();
            validate_tool(tool, &args).unwrap();
            args["op"] = json!(tool.strip_prefix("hive_").unwrap());
            validate_tool(
                "hive_batch",
                &json!({"request_id":"batch","operations":[args]}),
            )
            .unwrap();
        }
    }
    for (attachments, code, field) in [
        (json!(null), "invalid_value", "/attachments"),
        (json!([false]), "invalid_path", "/attachments/0"),
        (json!(["relative-secret"]), "invalid_path", "/attachments/0"),
        (json!(["/secret\0"]), "invalid_path", "/attachments/0"),
        (json!(vec!["/secret"; 11]), "out_of_range", "/attachments"),
    ] {
        let args = json!({"body":"Review","request_id":"r","attachments":attachments});
        assert!(
            !diagnostic("hive_spawn", &args, code, field)
                .to_string()
                .contains("secret")
        );
    }
}

#[test]
fn approval_fields_share_caps_and_never_echo_form_names() {
    for (field, value, code) in [
        ("message", json!(""), "invalid_value"),
        ("message", json!("é".repeat(8193)), "out_of_range"),
        ("title", json!("é".repeat(129)), "out_of_range"),
        (
            "schema",
            json!({"type":"object","properties":{"secret":{"type":"object"}}}),
            "invalid_value",
        ),
        ("destructive", Value::Null, "invalid_value"),
    ] {
        let mut args = json!({"message":"Proceed?","request_id":"r"});
        args[field] = value;
        assert!(
            !diagnostic("hive_ask", &args, code, &format!("/{field}"))
                .to_string()
                .contains("secret")
        );
    }
}

#[test]
fn every_advertised_property_and_example_is_covered() {
    fn descriptions(schema: &Value) {
        if let Some(properties) = schema["properties"].as_object() {
            for (name, property) in properties {
                assert!(
                    property["description"]
                        .as_str()
                        .is_some_and(|s| !s.is_empty()),
                    "missing description: {name}"
                );
                descriptions(property);
            }
        }
        if let Some(items) = schema.get("items") {
            descriptions(items);
        }
    }
    for tool in tools().as_array().unwrap() {
        let name = tool["name"].as_str().unwrap();
        let example: Value = serde_json::from_str(catalog::example(name)).unwrap();
        assert_eq!(example["name"], name);
        validate_tool(name, &example["arguments"]).unwrap();
        assert!(
            tool["description"]
                .as_str()
                .unwrap()
                .contains(catalog::example(name))
        );
        descriptions(&tool["inputSchema"]);
    }
    let batch = tools()
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "hive_batch")
        .unwrap();
    assert_eq!(
        batch["inputSchema"]["properties"]["operations"]["items"]["properties"]["target"]["description"],
        "Agent ID for send or terminate, or @parent / @child (exactly one retained direct child)."
    );
}
