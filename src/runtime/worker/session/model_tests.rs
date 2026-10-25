use super::validate_model;
use crate::runtime::{provider::protocol::Model, test_support::agent};
use serde_json::{Value, json};

#[test]
fn shared_model_grammar_does_not_replace_catalog_membership() {
    for model in ["sonnet[1m]", "provider/model:revision@alias"] {
        let entry = Model::parse(
            &json!({"model":model, "supportedReasoningEfforts":[{"reasoningEffort":"high"}]}),
        );
        assert!(validate_model(model, "high", &[entry]).is_ok());
        assert_eq!(
            validate_model(model, "high", &[]).unwrap_err().code(),
            "unknown_model"
        );
    }
    for (model, effort, code) in [
        ("bad name", "high", "unknown_model"),
        ("sonnet", "fast", "unsupported_effort"),
    ] {
        // Even an untrusted native catalog cannot bypass the shared grammar.
        let entry = Model::parse(
            &json!({"model":model, "supportedReasoningEfforts":[{"reasoningEffort":effort}]}),
        );
        assert_eq!(
            validate_model(model, effort, &[entry]).unwrap_err().code(),
            code
        );
    }
}
#[test]
fn catalog_matching_is_field_local() {
    let selection = agent().desired_selection();
    for catalog in [
        vec![],
        vec![Value::Null],
        vec![json!({"model":17})],
        vec![json!({"model":"different","supportedReasoningEfforts":[]})],
    ] {
        let models: Vec<_> = catalog.iter().map(Model::parse).collect();
        crate::test_support::assert_code!(
            validate_model(&selection.model, &selection.effort, &models),
            "unknown_model"
        );
    }
    for effort in [
        Value::Null,
        json!(false),
        json!([]),
        json!([{"reasoningEffort":3}]),
        json!([{"reasoningEffort":"high"}]),
    ] {
        crate::test_support::assert_code!(
            validate_model(
                &selection.model,
                &selection.effort,
                &[Model::parse(&json!({"model":"gpt-5.6-luna",
            "supportedReasoningEfforts":effort}))]
            ),
            "unsupported_effort"
        );
    }
    validate_model(
        &selection.model,
        &selection.effort,
        &[Model::parse(&json!({"model":"gpt-5.6-luna","extra":null,
        "supportedReasoningEfforts":[null,{"reasoningEffort":"low","extra":false}]}))],
    )
    .unwrap();
    for (entry, expected) in [
        (json!({"model":"gpt-5.6-luna"}), Some("unsupported_effort")),
        (
            json!({"model":"gpt-5.6-luna","supportedReasoningEfforts":[{"reasoningEffort":"low"}]}),
            None,
        ),
    ] {
        assert_eq!(
            validate_model(&selection.model, &selection.effort, &[Model::parse(&entry)])
                .err()
                .map(|error| error.code()),
            expected
        );
    }
}
