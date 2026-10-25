use super::*;
use crate::{
    Error,
    error::value,
    service::{input, mcp},
};
use hive_input::Code;
use serde_json::json;

#[test]
fn malformed_dates_keep_private_native_cause_through_typed_rejection() {
    use std::error::Error as _;
    let error = rejected(
        "comb_search",
        json!({"query":"q", "after":"private-marker"}),
    );
    let domain = error.source().unwrap();
    assert!(
        domain
            .source()
            .unwrap()
            .downcast_ref::<chrono::ParseError>()
            .is_some()
    );
    assert_eq!(error.input().unwrap().field(), "/arguments/after");
    assert!(
        !serde_json::to_string(&error)
            .unwrap()
            .contains("private-marker")
    );
}

fn rejected(name: &str, args: Value) -> Error {
    match mcp::operation(name, args, None) {
        Ok(_) => panic!("accepted invalid retrieval"),
        Err(error) => error,
    }
}

#[test]
fn missing_wrong_type_ranges_and_private_fields_are_distinct() -> crate::Result<()> {
    for (name, args, code, field) in [
        ("comb_search", json!({}), Code::MissingArgument, "query"),
        (
            "comb_search",
            json!({"query":false}),
            Code::InvalidValue,
            "query",
        ),
        (
            "comb_search",
            json!({"query":"  "}),
            Code::InvalidValue,
            "query",
        ),
        (
            "comb_search",
            json!({"query":"q","limit":0}),
            Code::OutOfRange,
            "limit",
        ),
        (
            "comb_search",
            json!({"query":"q","limit":101}),
            Code::OutOfRange,
            "limit",
        ),
        (
            "comb_search",
            json!({"query":"q","limit":18_446_744_073_709_551_616.0}),
            Code::OutOfRange,
            "limit",
        ),
        (
            "comb_search",
            json!({"query":"q","mode":null}),
            Code::InvalidValue,
            "mode",
        ),
    ] {
        check_invalid(name, args, code, field)?;
    }
    Ok(())
}

#[test]
fn other_retrieval_types_and_ranges_are_checked() -> crate::Result<()> {
    for (name, args, code, field) in [
        (
            "comb_list",
            json!({"limit":1001}),
            Code::OutOfRange,
            "limit",
        ),
        (
            "comb_list",
            json!({"unverified":null}),
            Code::InvalidValue,
            "unverified",
        ),
        (
            "comb_links",
            json!({"slug":"a","limit":null}),
            Code::InvalidValue,
            "limit",
        ),
        (
            "comb_backlinks",
            json!({"slug":"a","after":"../private-marker"}),
            Code::InvalidValue,
            "after",
        ),
        (
            "comb_graph",
            json!({"slug":"a","depth":-1}),
            Code::OutOfRange,
            "depth",
        ),
        (
            "comb_graph",
            json!({"slug":"a","depth":9}),
            Code::OutOfRange,
            "depth",
        ),
        (
            "comb_get",
            json!({"slug":"private-marker\n"}),
            Code::InvalidValue,
            "slug",
        ),
    ] {
        check_invalid(name, args, code, field)?;
    }
    Ok(())
}

fn check_invalid(name: &str, args: Value, code: Code, field: &str) -> crate::Result<()> {
    let error = rejected(name, args);
    let input = error.input().ok_or_else(Error::internal_serialization)?;
    assert_eq!(input.code(), code);
    assert_eq!(input.field(), format!("/arguments/{field}"));
    assert!(!input.expected().is_empty());
    assert_eq!(input.examples().len(), 1);
    assert!(!serde_json::to_string(&error)?.contains("private-marker"));
    assert!(!error.file_committed);
    assert!(error.request_id.is_none());
    Ok(())
}

#[test]
fn unknown_keys_report_their_container_without_echo() {
    let error = rejected("comb_list", json!({"private-marker":42}));
    assert_eq!(error.input().unwrap().field(), "/arguments");
    assert_eq!(error.input().unwrap().code(), Code::UnknownArgument);
    assert!(
        !serde_json::to_string(&error)
            .unwrap()
            .contains("private-marker")
    );
}

#[test]
fn retrieval_byte_bounds_dates_and_cursors_follow_domain_validators() {
    for (field, max) in [
        ("query", crate::search::limits::QUERY_BYTES),
        ("kind", crate::limits::KIND_BYTES),
        ("tag", crate::limits::LABEL_BYTES),
        ("prefix", crate::values::limits::SLUG_BYTES),
    ] {
        let mut args = json!({"query":"q"});
        args[field] = "x".repeat(max).into();
        assert!(mcp::operation("comb_search", args.clone(), None).is_ok());
        args[field] = "x".repeat(max + 1).into();
        assert_eq!(
            rejected("comb_search", args).input().unwrap().field(),
            format!("/arguments/{field}")
        );
    }
    for (after, before, valid) in [
        ("2026-01-01T01:00:00+01:00", "2026-01-01T00:00:00Z", true),
        ("2026-01-02T00:00:00Z", "2026-01-01T00:00:00Z", false),
    ] {
        assert_eq!(
            mcp::operation(
                "comb_search",
                json!({"query":"q","after":after,"before":before}),
                None
            )
            .is_ok(),
            valid
        );
    }
    for name in ["comb_list", "comb_links", "comb_backlinks"] {
        let mut args = json!({"after":"bad//cursor"});
        if name != "comb_list" {
            args["slug"] = "a".into();
        }
        assert_eq!(
            rejected(name, args).input().unwrap().field(),
            "/arguments/after"
        );
    }
}

#[test]
fn endpoint_values_defaults_and_nullable_options_remain_valid() -> crate::Result<()> {
    for (name, args) in [
        ("comb_search", json!({"query":"q","limit":1})),
        (
            "comb_search",
            json!({"query":"q","limit":100,"after":null,"kind":null}),
        ),
        ("comb_list", json!({"limit":null,"prefix":null})),
        ("comb_list", json!({"limit":1000,"after":null})),
        ("comb_links", json!({"slug":"a","limit":1,"after":null})),
        ("comb_backlinks", json!({"slug":"a","limit":1000})),
        ("comb_graph", json!({"slug":"a","depth":0})),
        ("comb_graph", json!({"slug":"a","depth":8})),
    ] {
        mcp::operation(name, args, None)?;
    }
    assert_eq!(
        value(mcp::operation("comb_graph", json!({"slug":"a"}), None)?)?["depth"],
        2
    );
    assert_eq!(
        value(mcp::operation("comb_search", json!({"query":"q"}), None)?)?["request"]["limit"],
        10
    );
    Ok(())
}

#[test]
fn socket_uses_the_same_validation_with_operation_pointers() {
    for (operation, pointer) in [
        (
            json!({"op":"search","request":{}}),
            "/operation/request/query",
        ),
        (
            json!({"op":"search","request":{"query":"q"},"mode":false}),
            "/operation/mode",
        ),
        (
            json!({"op":"list","request":{"limit":0}}),
            "/operation/request/limit",
        ),
        (json!({"op":"get","slug":"../bad"}), "/operation/slug"),
        (json!({"op":"graph","slug":"a"}), "/operation/depth"),
        (json!({"op":"search"}), "/operation/request"),
        (
            json!({"op":"links","slug":"a","incoming":null}),
            "/operation/incoming",
        ),
    ] {
        let request = json!({"protocol":1,"credential":{"comb_id":"12345678-1234-4234-8234-123456789abc", "id":"human", "secret":"private"}, "operation":operation});
        let Err(error) = input::request(&serde_json::to_vec(&request).unwrap()) else {
            panic!("accepted invalid socket request")
        };
        assert_eq!(error.input().unwrap().field(), pointer);
        let wire = serde_json::to_vec(&error).unwrap();
        let roundtrip: Error = serde_json::from_slice(&wire).unwrap();
        assert_eq!(roundtrip.input(), error.input());
    }
}
