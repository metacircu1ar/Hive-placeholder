use super::{Config, HttpConfig};
use crate::Error;

mod cases;
mod observations;

#[test]
fn config_json_yaml_and_errors_match_the_captured_contract() {
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("tests/contract.json")).unwrap();
    let actual = observations::all();
    let expected = expected.as_array().unwrap();
    let actual = actual.as_array().unwrap();
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected) {
        assert_eq!(
            actual, expected,
            "{} / {}",
            expected["name"], expected["format"]
        );
    }
}

#[test]
fn public_http_config_is_strict_and_serializes_as_the_flat_tagged_variant() {
    let (_, minimal) = cases::COMMON[0];
    let config: Config = serde_json::from_str(minimal).unwrap();
    let Config::Http(http) = config;
    assert_eq!(http.model, "model");
    assert_eq!(http.revision, "revision");
    assert_eq!(http.url, "https://example.test/embeddings");
    assert_eq!(http.dimensions, 3);
    assert_eq!(http.document_prefix, "");
    assert_eq!(http.query_prefix, "");
    assert_eq!(http.api_key_env, None);
    assert_eq!(http.timeout_seconds, 30);
    assert!(!http.request_dimensions);
    assert_eq!(http.tokenizer, None);
    assert_eq!(http.max_tokens, None);
    let mut object = serde_json::to_value(&http).unwrap();
    let reparsed: HttpConfig = serde_json::from_value(object.clone()).unwrap();
    assert_eq!(
        serde_json::to_string(&Config::Http(reparsed)).unwrap(),
        serde_json::to_string(&Config::Http(http)).unwrap()
    );
    object["unknown"] = serde_json::Value::Bool(true);
    assert!(serde_json::from_value::<HttpConfig>(object).is_err());
}

#[test]
fn file_read_resolves_assets_and_classifies_yaml_errors_as_config() {
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("tests/contract.json")).unwrap();
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("embedding.yaml");
    for &(name, text) in cases::YAML {
        let outcome = &expected
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == name && case["format"] == "yaml")
            .unwrap()["outcome"];
        std::fs::write(&path, text).unwrap();
        match Config::read(&path) {
            Ok(config) => {
                let mut expected: serde_json::Value =
                    serde_json::from_str(outcome["json"].as_str().unwrap()).unwrap();
                for key in ["directory", "tokenizer"] {
                    if let Some(asset) = expected.get(key).and_then(serde_json::Value::as_str)
                        && std::path::Path::new(asset).is_relative()
                    {
                        expected[key] = serde_json::json!(root.path().join(asset));
                    }
                }
                assert_eq!(serde_json::to_value(config).unwrap(), expected, "{name}");
            }
            Err(error) => {
                assert!(outcome["public"].is_object(), "{name}");
                assert_eq!(error.code, crate::ErrorCode::InvalidEmbeddingConfig);
                assert_eq!(
                    error.message,
                    Error::invalid_embedding_config_file().message
                );
                assert!(error.diagnostic().is_some());
            }
        }
    }
}
