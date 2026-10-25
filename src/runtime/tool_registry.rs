//! Compiled provider-facing tools and stable native request identities.
use super::api;
#[cfg(feature = "comb")]
use comb_protocol::service::mcp;
use serde_json::Value;
#[cfg(feature = "comb")]
use serde_json::json;
use sha2::{Digest, Sha256};
pub(super) fn tools(_catalog: crate::compatibility::AgentCatalog, reviewer: Option<bool>) -> Value {
    #[cfg(not(feature = "comb"))]
    let _ = reviewer;
    let catalog = api::tools().as_array().cloned().unwrap_or_default();
    #[cfg(feature = "comb")]
    let catalog = {
        let mut catalog = catalog;
        if let Some(reviewer) = reviewer {
            catalog.extend(mcp::tools(reviewer).into_iter().map(|tool| {
                json!({
                    "type":"function", "name":tool.name, "description":tool.description,
                    "inputSchema":tool.input_schema
                })
            }));
        }
        catalog
    };
    Value::Array(catalog)
}

pub(super) fn advertised(
    _catalog: crate::compatibility::AgentCatalog,
    name: &str,
    reviewer: Option<bool>,
) -> bool {
    #[cfg(feature = "comb")]
    if name.starts_with("comb_") {
        return reviewer
            .is_some_and(|reviewer| mcp::tools(reviewer).iter().any(|t| t.name == name));
    }
    #[cfg(not(feature = "comb"))]
    let _ = reviewer;
    api::tools()
        .as_array()
        .is_some_and(|tools| tools.iter().any(|t| t["name"] == name))
}

pub(super) fn request_id(execution: &str, turn: &str, native: &Value) -> String {
    let bytes = api::value((execution, turn, native)).to_string();
    format!("{:x}", Sha256::digest(bytes.as_bytes()))
}

#[cfg(all(test, not(feature = "comb")))]
#[test]
fn core_native_catalog_omits_comb_even_with_a_saved_binding() {
    let expected: Value = serde_json::from_str(include_str!("api/tool_schemas_core.json")).unwrap();
    for binding in [None, Some(false), Some(true)] {
        assert_eq!(tools(hive_input::compatibility::Exact, binding), expected);
        assert!(!advertised(
            hive_input::compatibility::Exact,
            "comb_get",
            binding
        ));
        assert!(!advertised(
            hive_input::compatibility::Exact,
            "hive_evidence",
            binding
        ));
        assert!(advertised(
            hive_input::compatibility::Exact,
            "hive_send",
            binding
        ));
    }
}

#[cfg(test)]
#[test]
fn saved_catalog_is_required_and_unsupported_catalogs_are_not_rebound() {
    let agent = crate::runtime::test_support::AgentBuilder::new("catalog").build();
    let original = serde_json::to_value(&agent).unwrap();
    assert_eq!(original["tool_catalog"], 1);
    crate::compatibility::contract(
        "agent-catalog.json",
        &serde_json::json!({"tool_catalog": agent.tool_catalog}),
    );
    for replacement in [
        None,
        Some(serde_json::json!(2)),
        Some(serde_json::json!(null)),
    ] {
        let mut raw = original.clone();
        if let Some(value) = replacement {
            raw["tool_catalog"] = value;
        } else {
            raw.as_object_mut().unwrap().remove("tool_catalog");
        }
        assert!(serde_json::from_value::<crate::runtime::domain::Agent>(raw).is_err());
    }
    let restored: crate::runtime::domain::Agent = serde_json::from_value(original).unwrap();
    assert_eq!(
        tools(restored.tool_catalog, None),
        tools(agent.tool_catalog, None)
    );
}
