use super::*;
use crate::{ErrorCode, error::value, search::Mode, service::mcp::tools};
use serde_json::json;

fn rejected(name: &str, arguments: Value, call_id: Option<&str>) -> Error {
    match operation(name, arguments, call_id) {
        Ok(_) => panic!("unexpectedly accepted {name}"),
        Err(error) => {
            assert!(!error.file_committed);
            assert!(error.request_id.is_none());
            error
        }
    }
}
fn assert_error(name: &str, arguments: Value, expected: Error) {
    let error = rejected(name, arguments, Some("host-call"));
    let native = std::error::Error::source(&error)
        .unwrap()
        .downcast_ref::<Error>()
        .unwrap();
    assert_eq!(native.code, expected.code);
    assert_eq!(native.message, expected.message);
    assert!(error.input().is_some());
}
fn structural(name: &str, arguments: Value) -> Error {
    let error = rejected(name, arguments, Some("host-call"));
    let native = std::error::Error::source(&error)
        .unwrap()
        .downcast_ref::<Error>()
        .unwrap();
    assert_eq!(native.code, ErrorCode::InvalidJson);
    assert_eq!(native.message, "Invalid JSON (Data, line 0, column 0).");
    assert!(error.input().is_some());
    error
}
fn consolidation() -> Value {
    json!({
        "note":"inbox/source", "target":"concepts/target",
        "note_hash":"a".repeat(64), "target_hash":"b".repeat(64),
        "summary":"Combined", "request_id":"explicit", "run":"review"
    })
}

fn typed(name: &str, arguments: Value, code: hive_input::Code, field: &str) {
    let error = rejected(name, arguments, Some("host-call"));
    let input = error.input().unwrap();
    assert_eq!(input.code(), code);
    assert_eq!(input.field(), field);
}

#[test]
fn envelopes_and_unknown_fields_are_actionable_without_echoing_input() {
    use hive_input::Code;
    typed("unknown", Value::Null, Code::InvalidValue, "/arguments");
    for name in ["unknown", "comb_remove"] {
        typed(name, json!({"op":null}), Code::InvalidValue, "/name");
    }
    for name in [
        "comb_links",
        "comb_backlinks",
        "comb_get",
        "comb_graph",
        "comb_search",
        "comb_list",
    ] {
        typed(
            name,
            json!({"op":null}),
            Code::UnknownArgument,
            "/arguments",
        );
    }
    for name in ["comb_put", "comb_consolidate"] {
        assert_error(name, json!({"op":null}), Error::op_argument_forbidden());
    }
}

#[test]
fn search_requires_query_and_checks_mode_before_body() -> Result<()> {
    use hive_input::Code;
    typed(
        "comb_search",
        json!({"mode":"wrong-mode-marker","query":false}),
        Code::InvalidValue,
        "/arguments/mode",
    );
    typed(
        "comb_search",
        json!({"mode":null}),
        Code::InvalidValue,
        "/arguments/mode",
    );
    typed(
        "comb_search",
        json!({"query":null}),
        Code::InvalidValue,
        "/arguments/query",
    );
    typed(
        "comb_search",
        json!({}),
        Code::MissingArgument,
        "/arguments/query",
    );
    typed(
        "comb_search",
        json!({"query":"q","after":"bad-date"}),
        Code::InvalidValue,
        "/arguments/after",
    );
    let Operation::Search { request, mode } = operation("comb_search", json!({"query":"q"}), None)?
    else {
        panic!("expected search");
    };
    assert_eq!(request.query, "q");
    assert_eq!(request.limit, crate::search::DEFAULT_SEARCH_LIMIT);
    assert_eq!(mode, Mode::Hybrid);
    Ok(())
}

#[test]
fn graph_links_and_list_keep_distinct_absent_and_null_rules() -> Result<()> {
    let graph = value(operation(
        "comb_graph",
        json!({"slug":"concepts/page"}),
        None,
    )?)?;
    assert_eq!(graph["depth"], 2);
    typed(
        "comb_graph",
        json!({"slug":"concepts/page","depth":null}),
        hive_input::Code::InvalidValue,
        "/arguments/depth",
    );
    typed(
        "comb_links",
        json!({"slug":"concepts/page","limit":null}),
        hive_input::Code::InvalidValue,
        "/arguments/limit",
    );
    for (name, incoming) in [("comb_links", false), ("comb_backlinks", true)] {
        let links = value(operation(
            name,
            json!({"slug":"concepts/page","after":null}),
            None,
        )?)?;
        assert_eq!(links["incoming"], incoming);
        assert_eq!(links["limit"], 100);
        assert_eq!(links["after"], Value::Null);
    }
    let list = value(operation(
        "comb_list",
        json!({"limit":null,"prefix":null}),
        None,
    )?)?;
    assert_eq!(list["request"]["limit"], Value::Null);
    typed(
        "comb_list",
        json!({"unverified":null}),
        hive_input::Code::InvalidValue,
        "/arguments/unverified",
    );
    Ok(())
}

#[test]
fn request_ids_distinguish_missing_null_and_explicit_and_preserve_host_text() -> Result<()> {
    let bare = json!({"text":"evidence"});
    let hosted = operation("comb_capture", bare.clone(), Some("host identity"))?;
    assert_eq!(hosted.request_id(), Some("host identity"));
    let explicit = operation(
        "comb_capture",
        json!({"text":"evidence","request_id":"explicit"}),
        Some("host"),
    )?;
    assert_eq!(explicit.request_id(), Some("explicit"));
    let standalone = operation("comb_capture", bare.clone(), None)?;
    assert!(
        standalone
            .request_id()
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
    );
    structural("comb_capture", json!({"text":"evidence","request_id":null}));
    structural(
        "comb_capture",
        json!({"text":"evidence","request_id":false}),
    );
    assert_error(
        "comb_capture",
        json!({"text":"evidence","request_id":""}),
        Error::invalid_request_id(),
    );
    assert_eq!(
        rejected("comb_capture", bare.clone(), Some("host\nidentity"))
            .input()
            .unwrap()
            .field(),
        "/arguments/request_id"
    );
    let oversized = "x".repeat(129);
    assert_eq!(
        rejected("comb_capture", bare, Some(&oversized))
            .input()
            .unwrap()
            .field(),
        "/arguments/request_id"
    );
    Ok(())
}

#[test]
fn optional_mutation_nulls_and_shape_before_domain_validation_stay_unchanged() -> Result<()> {
    let put = operation(
        "comb_put",
        json!({
            "slug":"concepts/page", "markdown":"text", "expected_hash":null, "run":null
        }),
        Some("host"),
    )?;
    let put = value(put)?;
    assert_eq!(put["expected_hash"], Value::Null);
    assert_eq!(put["run"], Value::Null);
    let append = value(operation(
        "comb_append",
        json!({
            "slug":"concepts/page", "text":"evidence", "source":"source",
            "summary":null, "expected_hash":null, "run":null
        }),
        Some("host"),
    )?)?;
    assert_eq!(append["summary"], Value::Null);
    assert_eq!(append["expected_hash"], Value::Null);
    assert_eq!(append["run"], Value::Null);
    structural(
        "comb_put",
        json!({"slug":"../bad","markdown":"text","request_id":null}),
    );
    assert_error(
        "comb_put",
        json!({"slug":"../bad","markdown":"text","request_id":""}),
        Error::invalid_slug(),
    );
    assert_error(
        "comb_put",
        json!({"slug":"concepts/page","markdown":"text","run":""}),
        Error::invalid_request(),
    );
    Ok(())
}

#[test]
fn consolidation_stages_missing_run_body_identity_then_run() -> Result<()> {
    assert_error(
        "comb_consolidate",
        json!({"note":false,"request_id":null}),
        Error::consolidation_run_missing(),
    );
    let mut input = consolidation();
    input["note"] = json!(23);
    input["request_id"] = Value::Null;
    input["run"] = Value::Null;
    let error = structural("comb_consolidate", input);
    assert!(
        error
            .diagnostic()
            .is_some_and(|detail| detail.contains("integer `23`"))
    );
    let mut input = consolidation();
    input["request_id"] = json!(17);
    input["run"] = Value::Null;
    let error = structural("comb_consolidate", input);
    assert!(
        error
            .diagnostic()
            .is_some_and(|detail| detail.contains("integer `17`"))
    );
    let mut input = consolidation();
    input["note"] = json!("../bad");
    input["run"] = Value::Null;
    structural("comb_consolidate", input);
    let mut input = consolidation();
    if let Value::Object(fields) = &mut input {
        fields.remove("request_id");
    }
    assert_eq!(
        operation("comb_consolidate", input, Some("host"))?.request_id(),
        Some("host")
    );
    Ok(())
}

#[test]
fn consolidation_domain_validation_order_is_shared_with_socket_operations() {
    let mut input = consolidation();
    input["note"] = json!("../bad");
    input["target"] = json!("../bad");
    input["note_hash"] = json!("bad");
    input["target_hash"] = json!("bad");
    input["request_id"] = json!("");
    input["run"] = json!("");
    assert_error("comb_consolidate", input.clone(), Error::invalid_slug());
    input["note"] = json!("inbox/source");
    assert_error("comb_consolidate", input.clone(), Error::invalid_slug());
    input["target"] = json!("concepts/target");
    let error = rejected("comb_consolidate", input.clone(), None);
    assert_eq!(error.input().unwrap().field(), "/arguments/note_hash");
    input["note_hash"] = json!("a".repeat(64));
    assert_eq!(
        rejected("comb_consolidate", input.clone(), None)
            .input()
            .unwrap()
            .field(),
        "/arguments/target_hash"
    );
    input["target_hash"] = json!("b".repeat(64));
    assert_error(
        "comb_consolidate",
        input.clone(),
        Error::invalid_request_id(),
    );
    input["request_id"] = json!("explicit");
    assert_error("comb_consolidate", input, Error::invalid_request());
}

#[test]
fn tool_and_socket_consolidation_share_actionable_value_errors() -> Result<()> {
    for (field, bad) in [
        ("note", "../bad"),
        ("target", "../bad"),
        ("note_hash", "bad"),
        ("target_hash", "bad"),
        ("request_id", ""),
        ("run", ""),
    ] {
        let mut args = consolidation();
        args[field] = json!(bad);
        let tool_error = rejected("comb_consolidate", args.clone(), Some("host"));
        let request = json!({
            "protocol": 1,
            "credential": {
                "comb_id":"12345678-1234-4234-8234-123456789abc",
                "id":"12345678-1234-4234-8234-123456789abc", "secret":"unused"
            },
            "claim":null,
            "operation": {
                "op":"consolidate", "request_id":args["request_id"], "run":args["run"],
                "request": {
                    "note":args["note"], "target":args["target"],
                    "note_hash":args["note_hash"], "target_hash":args["target_hash"],
                    "summary":args["summary"]
                }
            }
        });
        let bytes = serde_json::to_vec(&request)?;
        let Err(socket_error) = crate::service::input::request(&bytes) else {
            panic!("accepted invalid {field}")
        };
        assert_eq!(
            socket_error.input().unwrap().code(),
            tool_error.input().unwrap().code()
        );
        assert_eq!(
            socket_error.input().unwrap().field(),
            Write::Consolidate.pointer(field, false)
        );
        assert_eq!(
            tool_error.input().unwrap().field(),
            Write::Consolidate.pointer(field, true)
        );
    }
    Ok(())
}

#[test]
fn generated_catalogs_match_reviewed_contracts_without_recording() -> Result<()> {
    for (reviewer, expected) in [
        (
            false,
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../comb/tests/fixtures/contracts/mcp-catalog-false.json"
            ))
            .as_slice(),
        ),
        (
            true,
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../comb/tests/fixtures/contracts/mcp-catalog-true.json"
            ))
            .as_slice(),
        ),
    ] {
        assert_eq!(serde_json::to_vec_pretty(&tools(reviewer))?, expected);
    }
    Ok(())
}

fn sample(field: &str) -> Value {
    match field {
        "slug" | "target" | "prefix" => json!("concepts/page"),
        "note" => json!("inbox/source"),
        "expected_hash" | "note_hash" | "target_hash" => json!("a".repeat(64)),
        "after" | "before" => json!("2026-01-02T03:04:05Z"),
        "mode" => json!("hybrid"),
        "decision" => json!("accept"),
        "limit" | "depth" => json!(2),
        "recopy" | "unverified" => json!(false),
        _ => json!("text"),
    }
}

#[test]
fn every_advertised_field_is_decoded_by_its_tool_and_unknown_fields_stay_strict() -> Result<()> {
    for definition in tools(true) {
        let Value::Object(properties) = &definition.input_schema["properties"] else {
            panic!("missing properties");
        };
        let mut valid: Map<String, Value> = properties
            .keys()
            .map(|field| (field.clone(), sample(field)))
            .collect();
        let retrieval = Read::named(definition.name.strip_prefix("comb_").unwrap());
        if retrieval.is_some_and(|read| read != Read::Search) && valid.contains_key("after") {
            valid.insert("after".into(), json!("concepts/previous"));
        }
        operation(&definition.name, Value::Object(valid.clone()), Some("host"))?;
        for field in properties.keys() {
            let mut wrong = valid.clone();
            wrong.insert(field.clone(), json!([]));
            {
                typed(
                    &definition.name,
                    Value::Object(wrong),
                    hive_input::Code::InvalidValue,
                    &format!("/arguments/{field}"),
                );
            }
        }
        let mut extra = valid;
        extra.insert("unknown".into(), Value::Null);
        {
            typed(
                &definition.name,
                Value::Object(extra),
                hive_input::Code::UnknownArgument,
                "/arguments",
            );
        }
    }
    Ok(())
}
