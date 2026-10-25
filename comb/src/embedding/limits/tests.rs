use crate::{
    Error, ErrorCode,
    embedding::{
        Config, Engine, HttpConfig, Space, decode, normalize, tokenizer::Preflight, validate_query,
    },
};

fn space() -> Space {
    Space {
        backend: "test".into(),
        model: "test".into(),
        revision: "fixed".into(),
        dimensions: 1,
        document_prefix: String::new(),
        query_prefix: String::new(),
        preprocessing: "test".into(),
        normalization: "l2".into(),
    }
}

#[test]
fn space_dimensions_accept_both_endpoints_and_reject_adjacent_values() {
    for (dimensions, valid) in [(0, false), (1, true), (4096, true), (4097, false)] {
        let mut value = space();
        value.dimensions = dimensions;
        assert_eq!(value.validate().is_ok(), valid, "dimensions {dimensions}");
        if !valid {
            assert_eq!(
                value.validate().unwrap_err().message,
                Error::invalid_embedding_space().message
            );
        }
    }
}

#[test]
fn space_identity_and_prefix_limits_measure_bytes_not_characters() {
    for identity in ["x".repeat(4096), "é".repeat(2048)] {
        let mut value = space();
        value.model = identity;
        assert!(value.validate().is_ok());
        value.model.push('x');
        assert_eq!(
            value.validate().unwrap_err().code,
            ErrorCode::InvalidEmbeddingConfig
        );
    }
    for document in [false, true] {
        let mut value = space();
        let prefix = if document {
            &mut value.document_prefix
        } else {
            &mut value.query_prefix
        };
        *prefix = "é".repeat(512);
        assert!(value.validate().is_ok());
        if document {
            value.document_prefix.push('x');
        } else {
            value.query_prefix.push('x');
        }
        assert_eq!(
            value.validate().unwrap_err().message,
            Error::invalid_embedding_space().message
        );
    }
}

#[test]
fn query_boundary_is_shared_with_search_without_changing_error_context() {
    for query in ["x".repeat(4096), "é".repeat(2048)] {
        assert!(validate_query(&query).is_ok());
        assert!(
            crate::search::validate(&crate::search::Search {
                query: query.clone(),
                ..Default::default()
            })
            .is_ok()
        );
        let oversized = format!("{query}x");
        assert_eq!(
            validate_query(&oversized).unwrap_err().message,
            Error::invalid_query().message
        );
        assert!(
            crate::search::validate(&crate::search::Search {
                query: oversized,
                ..Default::default()
            })
            .is_err()
        );
    }
    for blank in ["", " \t\n"] {
        assert_eq!(
            validate_query(blank).unwrap_err().code,
            ErrorCode::InvalidSearch
        );
    }
}

#[test]
fn http_token_range_is_checked_before_opening_the_tokenizer() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing.json");
    for limit in [15, 16, 8192, 8193] {
        let error = Preflight::load(&missing, limit).err().unwrap();
        if [16, 8192].contains(&limit) {
            assert_eq!(error.code, ErrorCode::IoError);
        } else {
            assert_eq!(error.message, Error::http_tokenizer_token_limit().message);
        }
    }
}

fn http(timeout_seconds: u64, api_key_env: String) -> Config {
    Config::Http(HttpConfig {
        model: "test".into(),
        revision: "fixed".into(),
        url: "http://127.0.0.1:9/v1/embeddings".into(),
        dimensions: 1,
        document_prefix: String::new(),
        query_prefix: String::new(),
        api_key_env: Some(api_key_env),
        timeout_seconds,
        request_dimensions: false,
        tokenizer: None,
        max_tokens: None,
    })
}

#[test]
fn http_timeout_range_precedes_credential_lookup_at_both_endpoints() {
    let variable = format!("COMB_ABSENT_{}", uuid::Uuid::new_v4().simple());
    assert!(std::env::var_os(&variable).is_none());
    for timeout in [0, 1, 120, 121] {
        let error = Engine::load(http(timeout, variable.clone())).err().unwrap();
        if [1, 120].contains(&timeout) {
            assert_eq!(error.code, ErrorCode::EmbeddingCredentials);
        } else {
            assert_eq!(error.message, Error::unsafe_embedding_endpoint().message);
        }
    }
}

#[test]
fn normalization_retains_the_original_final_f32_rounding_and_signed_zero() {
    for (input, expected) in [
        (
            vec![3.0, 4.0, 0.0, -0.0],
            vec![0x3f19_999a, 0x3f4c_cccd, 0, 0x8000_0000],
        ),
        (
            vec![f32::from_bits(1), f32::from_bits(1)],
            vec![0x3f35_04f3; 2],
        ),
        (
            vec![f32::MAX, f32::MIN_POSITIVE, -0.0],
            vec![0x3f80_0000, 0, 0x8000_0000],
        ),
    ] {
        let dimensions = input.len();
        let output = normalize(input, dimensions).unwrap();
        assert_eq!(
            output.into_iter().map(f32::to_bits).collect::<Vec<_>>(),
            expected
        );
    }
}

#[test]
fn cached_norm_tolerance_preserves_adjacent_f32_boundary() {
    // Adjacent binary32 values straddle the existing squared-norm tolerance.
    let accepted = f32::from_bits(0x3f80_01a3);
    let rejected = f32::from_bits(0x3f80_01a4);
    assert_eq!(decode(&accepted.to_le_bytes(), 1).unwrap(), vec![accepted]);
    assert_eq!(
        decode(&rejected.to_le_bytes(), 1).unwrap_err().message,
        Error::cached_vector_norm().message
    );
}
