use super::{ParseError, parse};
use crate::{error::Error, external_mcp::catalog};
use serde_json::{Value, json};

fn failure(params: Value) -> (bool, Error) {
    match parse(params) {
        Err(ParseError::Protocol(error)) => (true, error),
        Err(ParseError::Arguments(error)) => (false, error),
        Ok(_) => panic!("accepted invalid tool call"),
    }
}
fn public_failure(params: Value) -> (bool, Value) {
    let (protocol, error) = failure(params);
    (protocol, crate::runtime::api::value(error))
}

#[test]
fn external_request_ids_are_private_and_bounded_but_bodies_keep_newlines() {
    for (id, accepted) in [
        ("retry 1".into(), true),
        ("é".repeat(64), true),
        ("é".repeat(65), false),
        ("x".repeat(129), false),
        ("private\nidentity".into(), false),
        ("private\u{85}identity".into(), false),
        (String::new(), false),
    ] {
        let result = super::parse(serde_json::json!({"name":"hive_call", "arguments":{
            "target":"a", "body":"line one\nline two", "request_id":id}}));
        assert_eq!(result.is_ok(), accepted);
        if let Err(ParseError::Arguments(error)) = result {
            assert!(!error.to_string().contains("private"));
        }
    }
}

#[test]
fn malformed_tool_arguments_keep_parser_causes_private() {
    use serde_json::json;
    use std::error::Error as _;

    for params in [
        json!({"name":12,"arguments":{}}),
        json!({"name":"hive_call","arguments":{
            "target":12,"body":"secret","request_id":"r"}}),
        json!({"name":"hive_wait_message","arguments":{"message_id":[]}}),
        json!({"name":"hive_output","arguments":{"id":false}}),
        json!({"name":"hive_wait_message","arguments":{"message_id":"m"},
            "_meta":{"progressToken":{"private":"secret"}}}),
    ] {
        let (_, error) = failure(params);
        assert!(matches!(error.source(), Some(source) if source.is::<serde_json::Error>()));
        let value = crate::runtime::api::value(error);
        assert_eq!(value["code"], "invalid_value");
        assert!(!value["field"].as_str().unwrap().is_empty());
        assert!(!value["expected"].as_str().unwrap().is_empty());
        assert!(!value.to_string().contains("secret"));
        assert!(value.get("received").is_none());
        for example in value["examples"].as_array().unwrap() {
            assert!(parse(serde_json::from_str(example.as_str().unwrap()).unwrap()).is_ok());
        }
    }
}

#[test]
fn external_call_rejects_delay() {
    assert!(
        super::parse(serde_json::json!({"name":"hive_call","arguments":{
        "target":"a","body":"x","request_id":"r","delay_seconds":1}}))
        .is_err()
    );
}

#[test]
fn external_attachments_are_bounded_absolute_paths_not_metadata() {
    use serde_json::json;
    for (attachments, accepted) in [
        (json!(["/missing/image.png"]), true),
        (json!([]), true),
        (json!(["relative/image.png"]), false),
        (json!(vec!["/missing"; 11]), false),
        (json!([{"path":"/missing","sha256":"untrusted"}]), false),
        (json!(null), false),
    ] {
        let result = super::parse(json!({"name":"hive_call","arguments":{
            "target":"a","body":"inspect","request_id":"r","attachments":attachments}}));
        assert_eq!(result.is_ok(), accepted, "{attachments}");
    }
}

#[test]
fn output_limit_contract_matches_utf8_page_bounds() {
    let id = "00000000-0000-4000-8000-000000000001";
    for (limit, accepted) in [
        (0, false),
        (3, false),
        (4, true),
        (65536, true),
        (65537, false),
    ] {
        assert_eq!(
            super::parse(
                serde_json::json!({"name":"hive_output", "arguments":{"id":id, "limit":limit}})
            )
            .is_ok(),
            accepted
        );
    }
    let catalog = serde_json::to_value(super::catalog()).unwrap();
    let output = catalog
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "hive_output")
        .unwrap();
    assert_eq!(output["inputSchema"]["properties"]["limit"]["minimum"], 4);
}

#[test]
fn malformed_wrapper_fields_use_protocol_errors() {
    for (params, code, field) in [
        (json!(null), "invalid_value", "/"),
        (json!([]), "invalid_value", "/"),
        (json!({}), "missing_argument", "/name"),
        (json!({"name":null}), "invalid_value", "/name"),
        (json!({"name":"secret-canary"}), "unknown_argument", "/name"),
        (
            json!({"name":"hive_call","arguments":null}),
            "invalid_value",
            "/arguments",
        ),
        (
            json!({"name":"hive_call","arguments":{},"_meta":{"progressToken":false}}),
            "invalid_value",
            "/_meta/progressToken",
        ),
    ] {
        let (protocol, error) = public_failure(params);
        assert!(protocol);
        assert_eq!(error["code"], code);
        assert_eq!(error["field"], field);
        assert!(!error.to_string().contains("secret-canary"));
    }
    for (name, sequence) in [
        ("hive_call", json!(["a", "b", "r", [], "next-turn", 45])),
        ("hive_wait_message", json!(["m", 45])),
        (
            "hive_output",
            json!(["00000000-0000-4000-8000-000000000001", 0, 65536]),
        ),
    ] {
        let (protocol, error) = public_failure(json!({"name":name,"arguments":sequence}));
        assert!(protocol);
        assert_eq!(error["field"], "/arguments");
    }
}

#[test]
fn absent_arguments_and_required_keys_are_tool_errors() {
    for name in [
        super::ExternalTool::Call,
        super::ExternalTool::WaitMessage,
        super::ExternalTool::Output,
    ] {
        let example: Value = serde_json::from_str(catalog::example(name)).unwrap();
        let mut absent = example.clone();
        absent.as_object_mut().unwrap().remove("arguments");
        let (protocol, error) = public_failure(absent);
        assert!(!protocol);
        assert_eq!(
            error["field"],
            format!("/arguments/{}", catalog::required(name)[0])
        );
        for key in catalog::required(name) {
            let mut params = example.clone();
            params["arguments"].as_object_mut().unwrap().remove(*key);
            let (protocol, error) = public_failure(params);
            assert!(!protocol);
            assert_eq!(error["code"], "missing_argument");
            assert_eq!(error["field"], format!("/arguments/{key}"));
        }
    }
}

#[test]
fn call_fields_have_precise_diagnostics_without_values() {
    let example: Value = serde_json::from_str(catalog::example(super::ExternalTool::Call)).unwrap();
    for (key, value, code, field) in [
        ("target", json!(false), "invalid_value", "/arguments/target"),
        ("target", json!(""), "out_of_range", "/arguments/target"),
        (
            "body",
            json!("é".repeat(32769)),
            "out_of_range",
            "/arguments/body",
        ),
        ("body", json!(null), "invalid_value", "/arguments/body"),
        (
            "request_id",
            json!("secret-canary\n"),
            "invalid_request_id",
            "/arguments/request_id",
        ),
        (
            "request_id",
            json!([]),
            "invalid_value",
            "/arguments/request_id",
        ),
        ("mode", json!("steer"), "invalid_value", "/arguments/mode"),
        (
            "mode",
            json!("secret-canary"),
            "invalid_value",
            "/arguments/mode",
        ),
        ("mode", json!(false), "invalid_value", "/arguments/mode"),
        ("timeout", json!(0), "out_of_range", "/arguments/timeout"),
        ("timeout", json!(601), "out_of_range", "/arguments/timeout"),
        (
            "timeout",
            json!(45.0),
            "invalid_value",
            "/arguments/timeout",
        ),
        ("timeout", json!(-1), "invalid_value", "/arguments/timeout"),
        ("secret-canary", json!(1), "unknown_argument", "/arguments"),
    ] {
        let mut params = example.clone();
        params["arguments"][key] = value;
        let (protocol, error) = public_failure(params);
        assert!(!protocol, "{key}");
        assert_eq!(error["code"], code, "{key}");
        assert_eq!(error["field"], field, "{key}");
        assert!(!error.to_string().contains("secret-canary"));
        assert!(error.get("received").is_none());
    }
}

#[test]
fn call_attachment_diagnostics_identify_only_safe_indices() {
    let example: Value = serde_json::from_str(catalog::example(super::ExternalTool::Call)).unwrap();
    for (key, value, code, field) in [
        (
            "attachments",
            json!(null),
            "invalid_value",
            "/arguments/attachments",
        ),
        (
            "attachments",
            json!(vec!["/secret-canary"; 11]),
            "out_of_range",
            "/arguments/attachments",
        ),
        (
            "attachments",
            json!(["/ok", false]),
            "invalid_path",
            "/arguments/attachments/1",
        ),
        (
            "attachments",
            json!(["/ok", "secret-canary"]),
            "invalid_path",
            "/arguments/attachments/1",
        ),
        (
            "attachments",
            json!(["/secret-canary\0"]),
            "invalid_path",
            "/arguments/attachments/0",
        ),
        (
            "attachments",
            json!([format!("/{}", "x".repeat(65536))]),
            "invalid_path",
            "/arguments/attachments/0",
        ),
    ] {
        let mut params = example.clone();
        params["arguments"][key] = value;
        let (protocol, error) = public_failure(params);
        assert!(!protocol, "{key}");
        assert_eq!(error["code"], code, "{key}");
        assert_eq!(error["field"], field, "{key}");
        assert!(!error.to_string().contains("secret-canary"));
        assert!(error.get("received").is_none());
    }
}

#[test]
fn observation_fields_are_typed_and_bounded() {
    for (name, key, value, code) in [
        (
            super::ExternalTool::WaitMessage,
            "message_id",
            json!(null),
            "invalid_value",
        ),
        (
            super::ExternalTool::WaitMessage,
            "message_id",
            json!(""),
            "out_of_range",
        ),
        (
            super::ExternalTool::WaitMessage,
            "timeout",
            json!(601),
            "out_of_range",
        ),
        (
            super::ExternalTool::Output,
            "id",
            json!("secret-canary"),
            "invalid_uuid",
        ),
        (
            super::ExternalTool::Output,
            "id",
            json!(false),
            "invalid_value",
        ),
        (
            super::ExternalTool::Output,
            "offset",
            json!(-1),
            "invalid_value",
        ),
        (
            super::ExternalTool::Output,
            "offset",
            json!(1.0),
            "invalid_value",
        ),
        (
            super::ExternalTool::Output,
            "limit",
            json!(3),
            "out_of_range",
        ),
        (
            super::ExternalTool::Output,
            "limit",
            json!(65537),
            "out_of_range",
        ),
        (
            super::ExternalTool::Output,
            "limit",
            json!(4.0),
            "invalid_value",
        ),
    ] {
        let mut params: Value = serde_json::from_str(catalog::example(name)).unwrap();
        params["arguments"][key] = value;
        let (protocol, error) = public_failure(params);
        assert!(!protocol);
        assert_eq!(error["code"], code);
        assert_eq!(error["field"], format!("/arguments/{key}"));
        assert!(!error.to_string().contains("secret-canary"));
    }
}

#[test]
fn diagnostic_precedence_is_independent_of_json_key_order() {
    for (first, second, field) in [
        (
            r#"{"body":false,"secret-canary":true,"request_id":null}"#,
            r#"{"request_id":null,"secret-canary":true,"body":false}"#,
            "/arguments/target",
        ),
        (
            r#"{"target":false,"body":false,"request_id":null,"secret-canary":true}"#,
            r#"{"secret-canary":true,"request_id":null,"body":false,"target":false}"#,
            "/arguments",
        ),
        (
            r#"{"target":false,"body":false,"request_id":null}"#,
            r#"{"request_id":null,"body":false,"target":false}"#,
            "/arguments/target",
        ),
        (
            r#"{"target":"a","body":"b","request_id":"r","attachments":["relative",false],"mode":null,"timeout":0}"#,
            r#"{"timeout":0,"mode":null,"attachments":["relative",false],"request_id":"r","body":"b","target":"a"}"#,
            "/arguments/attachments/0",
        ),
    ] {
        let errors: Vec<_> = [first,second].into_iter().map(|args| {
            public_failure(json!({"name":"hive_call", "arguments":serde_json::from_str::<Value>(args).unwrap()}))
        }).collect();
        assert_eq!(errors[0], errors[1]);
        assert_eq!(errors[0].1["field"], field);
    }
}

#[test]
fn defaults_and_examples_match_advertised_fields() {
    for name in [
        super::ExternalTool::Call,
        super::ExternalTool::WaitMessage,
        super::ExternalTool::Output,
    ] {
        let mut params: Value = serde_json::from_str(catalog::example(name)).unwrap();
        params["extra"] = json!({"opaque":true});
        for key in catalog::optional(name) {
            params["arguments"].as_object_mut().unwrap().remove(*key);
        }
        let call = parse(params).ok().unwrap();
        match call.operation {
            super::Operation::Call(args) => {
                assert_eq!(args.mode, super::DeliveryMode::NextTurn);
                assert!(args.attachments.is_empty());
                assert_eq!(args.timeout, 45);
            }
            super::Operation::Wait(args) => assert_eq!(args.timeout, 45),
            super::Operation::Output(args) => {
                assert_eq!(args.offset, 0);
                assert_eq!(args.limit, 65536);
            }
        }
    }
    let tools = crate::runtime::api::value(super::catalog());
    for tool in tools.as_array().unwrap() {
        let name = super::ExternalTool::parse(tool["name"].as_str().unwrap());
        let schema = &tool["inputSchema"];
        assert_eq!(schema["required"], json!(catalog::required(name)));
        let properties = schema["properties"].as_object().unwrap();
        assert_eq!(
            properties.len(),
            catalog::required(name).len() + catalog::optional(name).len()
        );
        for key in catalog::required(name)
            .iter()
            .chain(catalog::optional(name))
        {
            assert!(!properties[*key]["description"].as_str().unwrap().is_empty());
        }
        assert!(
            tool["description"]
                .as_str()
                .unwrap()
                .contains(catalog::example(name))
        );
    }
}
