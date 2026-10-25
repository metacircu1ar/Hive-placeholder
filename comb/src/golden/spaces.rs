use crate::embedding::{Config, Embedder, Engine, HttpConfig};
use crate::golden;

#[test]
fn http_space_fingerprints_include_transport_and_tokenizer_policies() {
    for (label, dimensions, tokenizer) in [
        ("default", false, false),
        ("dimensions", true, false),
        ("tokenizer", true, true),
    ] {
        let model = Engine::load(Config::Http(HttpConfig {
            model: "fixture".into(),
            revision: "exact-weights".into(),
            url: "http://127.0.0.1:8081/v1/embeddings".into(),
            dimensions: 2,
            document_prefix: String::new(),
            query_prefix: "Instruct: Find knowledge\nQuery:".into(),
            api_key_env: None,
            timeout_seconds: 60,
            request_dimensions: dimensions,
            tokenizer: tokenizer.then(|| {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/http-tokenizer/tokenizer.json")
            }),
            max_tokens: tokenizer.then_some(128),
        }))
        .unwrap();
        golden::json(
            &format!("space-http-{label}.json"),
            &(model.space(), model.space().fingerprint().unwrap()),
        );
    }
}
