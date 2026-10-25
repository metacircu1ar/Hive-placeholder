use super::*;
use serde_json::{Value, json};
use std::error::Error as _;

fn config(value: Value) -> Result<Config> {
    Ok(serde_json::from_value(value)?)
}
fn http() -> Value {
    json!({"backend":"http","model":"PRIVATE_MODEL","revision":"pin",
        "dimensions":2,"url":"http://127.0.0.1:9/v1/embeddings"})
}
#[test]
fn valid_boundaries_and_multiline_prefixes_require_no_assets_or_credentials() -> Result<()> {
    for dimensions in [1, 4096] {
        let mut value = http();
        value["dimensions"] = json!(dimensions);
        value["model"] = json!("x".repeat(4096));
        value["revision"] = json!("x".repeat(4096));
        value["document_prefix"] = json!("");
        value["query_prefix"] = json!("\n".repeat(1024));
        value["api_key_env"] = json!("MISSING_ENV_NOT_READ");
        for timeout in [1, 120] {
            value["timeout_seconds"] = json!(timeout);
            config(value.clone())?.validate_input()?;
        }
        value["tokenizer"] = json!("/unused-tokenizer.json");
        for tokens in [16, 8192] {
            value["max_tokens"] = json!(tokens);
            config(value.clone())?.validate_input()?;
        }
    }
    Ok(())
}
#[test]
fn pure_rejection_retains_private_native_causes_and_does_not_change_parse_or_engine() -> Result<()>
{
    let mut value = http();
    value["url"] = json!("PRIVATE_URL");
    let config = Config::parse(&serde_json::to_vec(&value)?, None)?;
    let before = serde_json::to_vec(&config)?;
    let error = config.validate_input().expect_err("bad URL");
    assert_eq!(
        error.input().expect("typed field").field(),
        "/configuration/url"
    );
    let cause = error
        .source()
        .and_then(|s| s.downcast_ref::<Error>())
        .expect("native cause");
    assert_eq!(cause.code, crate::ErrorCode::InvalidEmbeddingConfig);
    assert!(cause.source().is_some());
    assert!(!serde_json::to_string(&error)?.contains("PRIVATE"));
    assert_eq!(serde_json::to_vec(&config)?, before);
    let Err(error) = crate::embedding::Engine::load(config) else {
        panic!("native URL validator must reject");
    };
    assert_eq!(error.code, crate::ErrorCode::InvalidEmbeddingConfig);
    assert!(error.input().is_none());
    Ok(())
}
#[test]
fn setup_reports_invalid_http_scalars_at_its_config_option() -> Result<()> {
    let mut value = http();
    value["dimensions"] = json!(0);
    let config = config(value)?;
    assert_eq!(
        config
            .validate_input()
            .expect_err("bad dimensions")
            .input()
            .expect("field")
            .field(),
        "/configuration/dimensions"
    );
    let Err(error) = crate::setup::Prepared::from_config(
        crate::setup::Options {
            embedding_config: Some("/unused-config".into()),
            ..crate::setup::Options::default()
        },
        Some(config),
    ) else {
        panic!("setup validates the configuration");
    };
    let input = error.input().expect("setup field");
    assert_eq!(input.field(), "/options/embedding_config");
    assert!(input.expected().contains("1..4096"), "{}", input.expected());
    Ok(())
}
