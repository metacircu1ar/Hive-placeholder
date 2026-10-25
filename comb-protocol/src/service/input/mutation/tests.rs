use super::*;
use crate::{
    Result,
    service::{
        Operation,
        input::{self, RawOperation},
        mcp,
    },
};
use hive_input::Code;
use serde_json::{Value, json};

fn socket(operation: Value) -> Result<Operation> {
    let request = json!({"protocol":1,"credential":{"comb_id":"12345678-1234-4234-8234-123456789abc","id":"12345678-1234-4234-8234-123456789abc","secret":"private-secret"},"operation":operation});
    input::request(&serde_json::to_vec(&request)?).map(|r| r.operation)
}
fn wire(mutation: Write, mut args: Value) -> Value {
    if mutation == Write::Consolidate {
        let id = args["request_id"].take();
        let run = args["run"].take();
        args.as_object_mut().unwrap().remove("request_id");
        args.as_object_mut().unwrap().remove("run");
        args = json!({"request":args,"request_id":id,"run":run});
    }
    args["op"] = mutation.name().into();
    args
}
#[test]
fn each_write_field_has_typed_structure_errors_on_tool_and_socket() {
    for mutation in [
        Write::Put,
        Write::Append,
        Write::Capture,
        Write::Consolidate,
        Write::Decide,
        Write::Promote,
        Write::Remove,
    ] {
        for field in mutation.fields() {
            let mut args = mutation.sample();
            args[field] = json!(["private-marker"]);
            let error = socket(wire(mutation, args.clone())).err().unwrap();
            assert_eq!(
                error.input().unwrap().field(),
                mutation.pointer(field, false)
            );
            assert_eq!(error.input().unwrap().code(), Code::InvalidValue);
            assert!(
                !serde_json::to_string(&error)
                    .unwrap()
                    .contains("private-marker")
            );
            assert!(std::error::Error::source(&error).is_some());
            if mutation != Write::Remove {
                let error =
                    mcp::operation(&format!("comb_{}", mutation.name()), args, Some("host"))
                        .err()
                        .unwrap();
                assert_eq!(
                    error.input().unwrap().field(),
                    mutation.pointer(field, true)
                );
                assert_eq!(error.input().unwrap().code(), Code::InvalidValue);
            }
        }
    }
}
#[test]
fn duplicate_socket_keys_remain_rejected_even_when_value_map_is_valid() {
    let value = wire(Write::Capture, Write::Capture.sample());
    let operation = serde_json::to_string(&value)
        .unwrap()
        .replace("\"text\":", "\"text\":\"private-duplicate\",\"text\":");
    let bytes = format!(
        "{{\"protocol\":1,\"credential\":{{\"comb_id\":\"12345678-1234-4234-8234-123456789abc\",\"id\":\"12345678-1234-4234-8234-123456789abc\",\"secret\":\"private\"}},\"operation\":{operation}}}"
    );
    let error = input::request(bytes.as_bytes()).err().unwrap();
    assert_eq!(error.input().unwrap().field(), "/operation");
    assert!(!serde_json::to_string(&error).unwrap().contains("private"));
}
#[test]
fn examples_decode_and_unknown_fields_never_echo_the_key() {
    for mutation in [
        Write::Put,
        Write::Append,
        Write::Capture,
        Write::Consolidate,
        Write::Decide,
        Write::Promote,
        Write::Remove,
    ] {
        let args = mutation.sample();
        socket(wire(mutation, args.clone())).unwrap();
        let mut wrong = args;
        wrong["private/unknown"] = Value::Null;
        let error = socket(wire(mutation, wrong)).err().unwrap();
        let expected = if mutation == Write::Consolidate {
            "/operation/request"
        } else {
            "/operation"
        };
        assert_eq!(error.input().unwrap().field(), expected);
        assert_eq!(error.input().unwrap().code(), Code::UnknownArgument);
        assert!(!serde_json::to_string(&error).unwrap().contains("private"));
    }
}
#[test]
fn native_value_causes_are_retained_under_typed_errors() {
    let raw: RawOperation = serde_json::from_value(
        json!({"op":"put","slug":"../private","markdown":"text","request_id":"id"}),
    )
    .unwrap();
    let error = operation(raw, true).err().unwrap();
    assert_eq!(error.input().unwrap().field(), "/arguments/slug");
    assert!(std::error::Error::source(&error).is_some());
    assert!(!serde_json::to_string(&error).unwrap().contains("private"));
}

#[test]
fn required_null_and_nullable_fields_match_native_envelopes_and_schema() {
    for mutation in [
        Write::Put,
        Write::Append,
        Write::Capture,
        Write::Consolidate,
        Write::Decide,
        Write::Promote,
    ] {
        for field in mutation.fields() {
            let mut args = mutation.sample();
            args[field] = Value::Null;
            let tool = mcp::operation(&format!("comb_{}", mutation.name()), args.clone(), None);
            let socket = socket(wire(mutation, args));
            assert_eq!(
                tool.is_ok(),
                mutation.nullable(field),
                "{} {field}",
                mutation.name()
            );
            assert_eq!(
                socket.is_ok(),
                mutation.nullable(field),
                "{} {field}",
                mutation.name()
            );
        }
    }
    for definition in mcp::tools(true) {
        let properties = definition.input_schema["properties"].as_object().unwrap();
        assert!(
            properties
                .values()
                .all(|p| p["description"].as_str().is_some_and(|d| !d.is_empty()))
        );
        assert!(
            definition.input_schema["examples"]
                .as_array()
                .is_some_and(|v| !v.is_empty())
        );
    }
}

#[test]
fn native_accepted_shapes_are_not_blamed_for_another_invalid_field() {
    let mut args = Write::Decide.sample();
    args["decision"] = json!({"accept":null});
    mcp::operation("comb_decide", args.clone(), None).unwrap();
    socket(wire(Write::Decide, args.clone())).unwrap();
    args["reason"] = json!(false);
    let tool = mcp::operation("comb_decide", args.clone(), None)
        .err()
        .unwrap();
    let native = socket(wire(Write::Decide, args)).err().unwrap();
    assert_eq!(tool.input().unwrap().field(), "/arguments/reason");
    assert_eq!(native.input().unwrap().field(), "/operation/reason");
    let body = json!([
        "inbox/note",
        "concepts/page",
        "a".repeat(64),
        "b".repeat(64),
        "Summary",
        false
    ]);
    let mut operation = json!({"op":"consolidate","request":body,"request_id":"id","run":"review"});
    socket(operation.clone()).unwrap();
    operation["request_id"] = Value::Null;
    assert_eq!(
        socket(operation).err().unwrap().input().unwrap().field(),
        "/operation/request_id"
    );
}

#[test]
fn nonobject_write_errors_offer_the_submitted_tool_example() {
    for name in [
        "put",
        "append",
        "capture",
        "consolidate",
        "decide",
        "promote",
    ] {
        let name = format!("comb_{name}");
        let error = mcp::operation(&name, Value::Null, None).err().unwrap();
        let input = error.input().unwrap();
        assert_eq!(input.field(), "/arguments");
        assert!(input.examples()[0].starts_with(&format!("{name} ")));
    }
}
