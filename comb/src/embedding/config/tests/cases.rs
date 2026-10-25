//! Inputs shared by the temporary old-parser probe and current contract test.
pub(super) const COMMON: &[(&str, &str)] = &[
    (
        "http-defaults",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":3}"#,
    ),
    (
        "http-full",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":3,"document_prefix":"document: ","query_prefix":"query:\n","api_key_env":"EMBEDDING_KEY","timeout_seconds":7,"request_dimensions":true,"tokenizer":"assets/tokenizer.json","max_tokens":42}"#,
    ),
    (
        "http-explicit-null-optionals",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":3,"api_key_env":null,"tokenizer":null,"max_tokens":null}"#,
    ),
    (
        "http-deferred-validation",
        r#"{"backend":"http","model":"","revision":"","url":"not a URL","dimensions":0,"timeout_seconds":0,"tokenizer":"missing","max_tokens":0}"#,
    ),
    (
        "removed-local-backend",
        r#"{"backend":"local","model":"local","directory":"assets","dimensions":3}"#,
    ),
    ("empty-map", r"{}"),
    ("null", r"null"),
    ("sequence", r"[]"),
    ("scalar", r#""http""#),
    ("missing-backend", r#"{"model":"model"}"#),
    ("unknown-backend", r#"{"backend":"unknown"}"#),
    ("null-backend", r#"{"backend":null}"#),
    ("numeric-backend", r#"{"backend":3}"#),
    ("backend-only", r#"{"backend":"http"}"#),
    (
        "duplicate-backend",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":3,"backend":"http"}"#,
    ),
    (
        "conflicting-backend",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":3,"backend":"local"}"#,
    ),
    (
        "missing-model",
        r#"{"backend":"http","revision":"revision","url":"https://example.test/embeddings","dimensions":3}"#,
    ),
    (
        "missing-revision",
        r#"{"backend":"http","model":"model","url":"https://example.test/embeddings","dimensions":3}"#,
    ),
    (
        "missing-url",
        r#"{"backend":"http","model":"model","revision":"revision","dimensions":3}"#,
    ),
    (
        "missing-dimensions",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings"}"#,
    ),
    (
        "wrong-model",
        r#"{"backend":"http","model":null,"revision":"revision","url":"https://example.test/embeddings","dimensions":3}"#,
    ),
    (
        "wrong-revision",
        r#"{"backend":"http","model":"model","revision":7,"url":"https://example.test/embeddings","dimensions":3}"#,
    ),
    (
        "wrong-url",
        r#"{"backend":"http","model":"model","revision":"revision","url":[],"dimensions":3}"#,
    ),
    (
        "wrong-dimensions",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":"3"}"#,
    ),
    (
        "wrong-document_prefix",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":3,"document_prefix":null}"#,
    ),
    (
        "wrong-query_prefix",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":3,"query_prefix":null}"#,
    ),
    (
        "wrong-api_key_env",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":3,"api_key_env":7}"#,
    ),
    (
        "wrong-timeout_seconds",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":3,"timeout_seconds":null}"#,
    ),
    (
        "wrong-request_dimensions",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":3,"request_dimensions":null}"#,
    ),
    (
        "wrong-tokenizer",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":3,"tokenizer":{}}"#,
    ),
    (
        "wrong-max_tokens",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":3,"max_tokens":1.5}"#,
    ),
    (
        "negative-dimensions",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":-1}"#,
    ),
    (
        "fractional-dimensions",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":1.5}"#,
    ),
    (
        "overflow-dimensions",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":18446744073709551616}"#,
    ),
    (
        "negative-timeout",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":3,"timeout_seconds":-1}"#,
    ),
    (
        "overflow-timeout",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":3,"timeout_seconds":18446744073709551616}"#,
    ),
    (
        "boolean-timeout",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":3,"timeout_seconds":true}"#,
    ),
    (
        "string-flag",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":3,"request_dimensions":"true"}"#,
    ),
    (
        "negative-max-tokens",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":3,"max_tokens":-1}"#,
    ),
    (
        "unknown-first",
        r#"{"unknown":true,"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":3}"#,
    ),
    (
        "unknown-last",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":3,"unknown":true}"#,
    ),
    (
        "unknown-and-missing",
        r#"{"backend":"http","unknown":true}"#,
    ),
    (
        "invalid-then-unknown",
        r#"{"backend":"http","model":7,"unknown":true}"#,
    ),
    (
        "unknown-then-invalid",
        r#"{"backend":"http","unknown":true,"model":7}"#,
    ),
    (
        "missing-and-invalid",
        r#"{"backend":"http","dimensions":"bad"}"#,
    ),
    (
        "duplicate-model",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":3,"model":"second"}"#,
    ),
    (
        "duplicate-invalid-model",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":3,"model":7}"#,
    ),
    (
        "invalid-first-duplicate-model",
        r#"{"backend":"http","model":7,"model":"second"}"#,
    ),
    (
        "duplicate-prefix",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":3,"query_prefix":"one","query_prefix":"two"}"#,
    ),
    (
        "duplicate-null-optional",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":3,"api_key_env":null,"api_key_env":"KEY"}"#,
    ),
    (
        "duplicate-max-tokens",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":3,"max_tokens":null,"max_tokens":5}"#,
    ),
    ("truncated", r#"{"backend":"http","model":"#),
    (
        "trailing-input",
        r#"{"backend":"http","model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":3} false"#,
    ),
    ("http-sequence-tag-only", r#"["http"]"#),
    ("http-sequence-short", r#"["http","model"]"#),
    (
        "http-sequence-defaults",
        r#"["http","model","revision","https://example.test/embeddings",3]"#,
    ),
    (
        "http-sequence-full",
        r#"["http","model","revision","https://example.test/embeddings",3,"","",null,30,false,null,null]"#,
    ),
    (
        "http-sequence-extra",
        r#"["http","model","revision","https://example.test/embeddings",3,"","",null,30,false,null,null,true]"#,
    ),
    (
        "http-sequence-map",
        r#"["http",{"model":"model","revision":"revision","url":"https://example.test/embeddings","dimensions":3}]"#,
    ),
];

pub(super) const YAML: &[(&str, &str)] = &[
    (
        "yaml-http-defaults",
        r"backend: http
model: model
revision: revision
url: https://example.test/embeddings
dimensions: 3
",
    ),
    (
        "yaml-invalid-dimensions",
        r"backend: http
model: model
revision: revision
url: https://example.test/embeddings
dimensions: wrong
",
    ),
    (
        "yaml-unknown-before-invalid",
        r"backend: http
unknown: true
model: []
",
    ),
    (
        "yaml-invalid-before-unknown",
        r"backend: http
model: []
unknown: true
",
    ),
    (
        "yaml-duplicate-model",
        r"backend: http
model: one
revision: revision
url: https://example.test/embeddings
dimensions: 3
model: two
",
    ),
    (
        "yaml-duplicate-backend",
        r"backend: http
backend: local
model: model
",
    ),
    (
        "yaml-null-prefix",
        r"backend: http
model: model
revision: revision
url: https://example.test/embeddings
dimensions: 3
document_prefix: null
",
    ),
    (
        "yaml-wrong-optional",
        r"backend: http
model: model
revision: revision
url: https://example.test/embeddings
dimensions: 3
api_key_env: [KEY]
",
    ),
    (
        "yaml-broken-sequence",
        r"backend: http
model: [broken
",
    ),
    (
        "yaml-multiple-documents",
        r"backend: http
---
backend: local
",
    ),
    (
        "yaml-alias",
        r"backend: http
model: &identity model
revision: *identity
url: https://example.test/embeddings
dimensions: 3
",
    ),
    (
        "yaml-merge-key",
        r"backend: http
<<: {model: model, revision: revision}
url: https://example.test/embeddings
dimensions: 3
",
    ),
    (
        "yaml-nonfinite-dimensions",
        r"backend: http
model: model
revision: revision
url: https://example.test/embeddings
dimensions: .nan
",
    ),
];
