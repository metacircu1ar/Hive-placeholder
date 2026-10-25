use super::*;
use serde_json::json;
fn schema() -> Value {
    serde_json::from_str(include_str!("../../../tests/fixtures/codex-schema.json")).unwrap()
}
#[test]
fn advertised_capabilities_require_methods_fields_and_types() {
    let original = schema();
    assert!(inspect(&original).unwrap().require_branching().is_ok());
    for method in [
        "thread/backgroundTerminals/list",
        "thread/backgroundTerminals/clean",
    ] {
        let mut value = original.clone();
        value["oneOf"]
            .as_array_mut()
            .unwrap()
            .retain(|node| node["properties"]["method"]["enum"][0] != method);
        assert!(inspect(&value).is_err());
    }
    for name in ["dynamicTools", "allowProviderModelFallback"] {
        let mut value = original.clone();
        value["definitions"]["ThreadStartParams"]["properties"]
            .as_object_mut()
            .unwrap()
            .remove(name);
        assert!(inspect(&value).is_err());
    }
    let mut value = original.clone();
    value["definitions"]["ThreadStartParams"]["properties"]["allowProviderModelFallback"]["type"] =
        json!("string");
    assert!(inspect(&value).is_err());
    for name in ["lastTurnId", "ephemeral", "excludeTurns"] {
        let mut value = original.clone();
        value["definitions"]["ThreadForkParams"]["properties"]
            .as_object_mut()
            .unwrap()
            .remove(name);
        assert!(inspect(&value).unwrap().require_branching().is_err());
    }
}
#[test]
fn dangling_external_cyclic_and_ambiguous_method_schemas_are_refused() {
    for reference in [
        "file:///etc/passwd",
        "https://example.invalid/schema",
        "../schema.json",
        "#/definitions/missing",
        "#/definitions/ThreadStartParams",
    ] {
        let mut value = schema();
        value["definitions"]["ThreadStartParams"] = json!({"$ref":reference});
        assert!(inspect(&value).is_err());
    }
    let mut value = schema();
    let duplicate = value["oneOf"][0].clone();
    value["oneOf"].as_array_mut().unwrap().push(duplicate);
    assert!(inspect(&value).is_err());
}
#[test]
fn schema_read_is_bounded_and_refuses_symlinks() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("schema");
    std::fs::write(&file, serde_json::to_vec(&schema()).unwrap()).unwrap();
    assert!(read(&file).is_ok());
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    assert!(read(&link).is_err());
    std::fs::File::create(&file)
        .unwrap()
        .set_len(SCHEMA_LIMIT + 1)
        .unwrap();
    assert!(read(&file).is_err());
}

#[test]
fn function_tools_require_matching_discriminator_and_supplied_required_fields() {
    for replacement in [
        json!([]),
        json!([{"properties":{"type":{"enum":["namespace"]}}}]),
    ] {
        let mut value = schema();
        value["definitions"]["DynamicToolSpec"]["oneOf"] = replacement;
        assert!(inspect(&value).is_err());
    }
    let mut value = schema();
    value["definitions"]["DynamicToolSpec"]["oneOf"][0]["required"]
        .as_array_mut()
        .unwrap()
        .push(json!("newRequiredInput"));
    assert!(inspect(&value).is_err());
    for input in [
        json!(false),
        json!({"type":"string"}),
        json!({"$ref":"https://example.invalid/input"}),
    ] {
        let mut value = schema();
        value["definitions"]["DynamicToolSpec"]["oneOf"][0]["properties"]["inputSchema"] = input;
        assert!(inspect(&value).is_err());
    }
    let original = schema();
    for contents in [
        include_str!("../../runtime/api/tool_schemas.json"),
        include_str!("../../runtime/api/tool_schemas_core.json"),
    ] {
        let catalog: Value = serde_json::from_str(contents).unwrap();
        for tool in catalog.as_array().unwrap() {
            assert_eq!(tool["type"], "function");
            for name in ["name", "description"] {
                assert!(tool[name].is_string());
            }
            assert!(tool["inputSchema"].is_object());
        }
    }
    assert!(inspect(&original).is_ok());
}

#[test]
fn constraints_beside_references_unions_and_discriminators_are_not_ignored() {
    let mut value = schema();
    value["oneOf"][0]["properties"]["params"]["required"] = json!(["confirmation"]);
    assert!(inspect(&value).is_err());
    let mut value = schema();
    value["definitions"]["DynamicToolSpec"]["not"] = json!({});
    assert!(inspect(&value).is_err());
    let mut value = schema();
    value["definitions"]["DynamicToolSpec"]["oneOf"][0]["properties"]["type"]["type"] =
        json!("number");
    assert!(inspect(&value).is_err());
    let mut value = schema();
    value["oneOf"][0]["required"] = json!(["id", "method", "params", "confirmation"]);
    assert!(inspect(&value).is_err());
}

#[test]
fn cleanup_rejects_unprovided_required_fields_and_unknown_constraints() {
    for name in [
        "ThreadBackgroundTerminalsCleanParams",
        "ThreadBackgroundTerminalsListParams",
    ] {
        let mut value = schema();
        value["definitions"][name]["required"] = json!(["threadId", "confirmation"]);
        assert!(inspect(&value).is_err());
        let mut value = schema();
        value["definitions"][name]["properties"]["threadId"]["pattern"] = json!("^other-");
        assert!(inspect(&value).is_err());
    }
}
