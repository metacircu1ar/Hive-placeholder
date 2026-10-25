use super::*;
use serde_json::json;

fn form(field: Value) -> Value {
    json!({"type":"object","properties":{"value":field},"required":["value"]})
}

#[test]
fn kinds_validate_values_defaults_and_required_fields() {
    for (field, good, bad) in [
        (
            json!({"type":"string","minLength":2,"maxLength":3}),
            json!("é🦀"),
            json!("x"),
        ),
        (
            json!({"type":"integer","minimum":-2,"maximum":3}),
            json!(-1),
            json!(1.5),
        ),
        (
            json!({"type":"number","minimum":-2.5,"maximum":3}),
            json!(-2.5),
            json!(3.1),
        ),
        (json!({"type":"boolean"}), json!(false), json!("false")),
        (
            json!({"type":"string","enum":["x","y"]}),
            json!("x"),
            json!("z"),
        ),
        (
            json!({"type":"string","oneOf":[{"const":"x","title":"X"}]}),
            json!("x"),
            json!("X"),
        ),
        (
            json!({"type":"array","items":{"type":"string","enum":["x","y"]},"minItems":1,"maxItems":2}),
            json!(["x", "y"]),
            json!([]),
        ),
        (
            json!({"type":"array","items":{"anyOf":[{"const":"x","title":"X"}]}}),
            json!(["x"]),
            json!(["z"]),
        ),
    ] {
        let schema = form(field.clone());
        assert!(valid_schema(&schema), "{schema}");
        assert!(valid_response(&schema, &json!({"value":good})), "{schema}");
        assert!(!valid_response(&schema, &json!({"value":bad})), "{schema}");
        assert!(!valid_response(&schema, &json!({})), "{schema}");
        assert!(!valid_response(
            &schema,
            &json!({"value":good,"extra":true})
        ));
        let mut with_default = field.clone();
        with_default["default"] = good;
        assert!(valid_schema(&form(with_default.clone())));
        with_default["default"] = bad;
        assert!(!valid_schema(&form(with_default)));
    }
    let schema = json!({"type":"object","properties":{"flag":{"type":"boolean","default":true}}});
    let empty = json!({});
    assert!(valid_response(&schema, &empty));
    assert_eq!(empty, json!({}), "defaults never mutate an answer");
    assert!(!valid_response(&schema, &Value::Null));
}

#[test]
fn unknown_keywords_invalid_shapes_and_contradictory_bounds_are_rejected() {
    for field in [
        json!({"type":"object","properties":{}}),
        json!({"type":"string","pattern":".*"}),
        json!({"type":"string","minLength":4,"maxLength":3}),
        json!({"type":"string","maxLength":-1}),
        json!({"type":"string","maxLength":1.5}),
        json!({"type":"string","title":7}),
        json!({"type":"string","format":"hostname"}),
        json!({"type":"string","$ref":"other"}),
        json!({"type":"number","minimum":2,"maximum":1}),
        json!({"type":"number","minimum":"2"}),
        json!({"type":"boolean","maxLength":1}),
        json!({"type":"array","items":{"type":"object"}}),
        json!({"type":"array","items":{"type":"string","enum":["x"]},"minItems":2,"maxItems":1}),
        json!({"type":"string","enum":[]}),
        json!({"type":"string","enum":["x","x"]}),
        json!({"type":"string","enum":[1]}),
        json!({"type":"string","enum":["x"],"oneOf":[{"const":"x","title":"X"}]}),
        json!({"type":"string","oneOf":[{"const":"x"}]}),
        json!({"type":"string","oneOf":[{"const":"x","title":"X","extra":true}]}),
        json!({"type":"array","items":{"anyOf":[{"const":"x","title":"X"}],"enum":["x"]}}),
    ] {
        assert!(!valid_schema(&form(field.clone())), "{field}");
    }
    for schema in [
        json!({"type":"object"}),
        json!({"type":"object","properties":{},"extra":true}),
        json!({"type":"object","properties":{},"required":["missing"]}),
        json!({"type":"object","properties":{"x":{"type":"boolean"}},"required":["x","x"]}),
        json!({"type":"object","properties":{},"required":null}),
        json!("{\"type\":\"object\",\"properties\":{}}"),
    ] {
        assert!(!valid_schema(&schema), "{schema}");
    }
}

#[test]
fn limits_apply_to_properties_choices_and_serialized_bytes() {
    let mut properties = Map::new();
    for i in 0..16 {
        properties.insert(i.to_string(), json!({"type":"boolean"}));
    }
    assert!(valid_schema(
        &json!({"type":"object","properties":properties})
    ));
    properties.insert("overflow".into(), json!({"type":"boolean"}));
    assert!(!valid_schema(
        &json!({"type":"object","properties":properties})
    ));
    let options: Vec<_> = (0..64).map(|n| n.to_string()).collect();
    assert!(valid_schema(&form(json!({"type":"string","enum":options}))));
    let options: Vec<_> = (0..65).map(|n| n.to_string()).collect();
    assert!(!valid_schema(&form(
        json!({"type":"string","enum":options})
    )));
    assert!(!valid_schema(&form(
        json!({"type":"string","description":"a".repeat(SCHEMA_BYTES)})
    )));
    assert!(!valid_response(
        &form(json!({"type":"string"})),
        &json!({"value":"a".repeat(RESPONSE_BYTES)})
    ));
}

#[test]
fn integer_bounds_do_not_round_through_floating_point() {
    let schema = form(json!({"type":"integer","maximum":9_007_199_254_740_992_u64}));
    assert!(!valid_response(
        &schema,
        &json!({"value":9_007_199_254_740_993_u64})
    ));
    let schema = form(json!({"type":"integer","minimum":u64::MAX,"maximum":u64::MAX}));
    assert!(valid_response(&schema, &json!({"value":u64::MAX})));
    assert!(!valid_response(
        &schema,
        &json!({"value":18_446_744_073_709_551_616.0})
    ));
    let schema = form(json!({"type":"integer","minimum":-2.5,"maximum":2.5}));
    assert!(valid_response(&schema, &json!({"value":-2})));
    assert!(!valid_response(&schema, &json!({"value":-3})));
    assert!(valid_response(&schema, &json!({"value":2.0})));
    assert!(!valid_response(&schema, &json!({"value":3})));
}

#[test]
fn formats_check_calendar_offsets_and_simple_contact_syntax() {
    for (format, good, bad) in [
        ("date", "2024-02-29", "2023-02-29"),
        ("date", "2000-02-29", "1900-02-29"),
        (
            "date-time",
            "2026-10-08T23:59:59.123+04:00",
            "2026-10-08T24:00:00Z",
        ),
        ("date-time", "2026-10-08t00:00:00z", "2026-10-08T00:00:00"),
        (
            "date-time",
            "2026-10-08T00:00:00-00:00",
            "2026-10-08T00:00:00.Z",
        ),
        ("email", "x@y", "example.com"),
        ("uri", "urn:example:value", "relative/path"),
        (
            "uri",
            "https://example.com/a%20b",
            "https://example.com/a b",
        ),
        ("uri", "mailto:x@example.com", "https://example.com/%zz"),
    ] {
        let schema = form(json!({"type":"string","format":format}));
        assert!(valid_response(&schema, &json!({"value":good})), "{good}");
        assert!(!valid_response(&schema, &json!({"value":bad})), "{bad}");
    }
}

#[test]
fn leap_seconds_are_rejected_in_values_and_defaults() {
    let mut schema =
        json!({"type":"object","properties":{"time":{"type":"string","format":"date-time"}}});
    assert!(!valid_response(
        &schema,
        &json!({"time":"2026-10-08T12:34:60Z"})
    ));
    schema["properties"]["time"]["default"] = json!("2026-10-08T12:34:60Z");
    assert!(!valid_schema(&schema));
}

#[test]
fn decimal_json_values_follow_the_parsers_binary64_domain() {
    let schema = form(json!({"type":"integer","minimum":1,"maximum":1}));
    let data: Value =
        serde_json::from_str(r#"{"value":1.00000000000000001}"#).expect("decimal JSON");
    assert!(valid_response(&schema, &data));
    assert!(!valid_response(&schema, &json!({"value":1.5})));
}
