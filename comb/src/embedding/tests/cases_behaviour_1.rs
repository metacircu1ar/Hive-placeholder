//! Behaviour regressions.
use super::*;

#[test]
fn hybrid_propagates_query_and_resident_poison_instead_of_text_fallback() {
    let (_root, mut comb, mut mock) = setup();
    put(&mut comb, "concepts/cat", "Cat", "feline");
    fill(&mut comb, &mut mock);
    let request = request("feline");
    let query = query(&comb, &mut mock, "feline");
    let cache = std::sync::Arc::new(
        crate::index::cache::Resident::new(query.lease.clone(), 1024, comb.health.clone()).unwrap(),
    );
    comb.index = comb.index.with_vectors(Some(cache.clone()));
    assert_eq!(
        comb.index
            .retrieve(
                &request,
                Mode::Hybrid,
                QueryVector::Unavailable(Error::shared_state_poisoned())
            )
            .err()
            .unwrap()
            .code,
        crate::ErrorCode::RecoveryRequired
    );
    assert_eq!(
        comb.index
            .retrieve(
                &request,
                Mode::Hybrid,
                QueryVector::Unavailable(Error::embeddings_busy())
            )
            .unwrap()
            .fallback,
        Some(crate::ErrorCode::EmbeddingsBusy)
    );
    cache.poison_on_next_read();
    assert_eq!(
        comb.index
            .retrieve(&request, Mode::Hybrid, QueryVector::Vector(query))
            .err()
            .unwrap()
            .code,
        crate::ErrorCode::RecoveryRequired
    );
    assert_eq!(
        comb.ready().err().unwrap().code,
        crate::ErrorCode::RecoveryRequired
    );
}

#[test]
fn blocked_query_work_does_not_hold_comb_and_queries_recheck_activation() {
    let (_root, mut comb, mut mock) = setup();
    put(&mut comb, "concepts/cat", "Cat", "feline");
    fill(&mut comb, &mut mock);
    for aba in [false, true] {
        comb.activate_embeddings(mock.space(), true).unwrap();
        let lease = comb.index().unwrap().embedding_lease(mock.space()).unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            run_query(
                &mut Blocked {
                    inner: mock_embedder("a"),
                    started: started_tx,
                    resume: resume_rx,
                },
                lease,
                "kitten",
            )
            .unwrap()
        });
        started_rx.recv().unwrap();
        // This mutation and equal-dimensional switch finish before inference is released.
        put(&mut comb, "concepts/other", "Other", "canine");
        comb.activate_embeddings(mock_embedder("b").space(), true)
            .unwrap();
        if aba {
            comb.activate_embeddings(mock.space(), true).unwrap();
        }
        resume_tx.send(()).unwrap();
        let late = handle.join().unwrap();
        let result = comb
            .index()
            .unwrap()
            .retrieve(&request("kitten"), Mode::Hybrid, QueryVector::Vector(late))
            .unwrap();
        assert_eq!(result.mode, Mode::Text);
        assert_eq!(
            result.fallback,
            Some(crate::ErrorCode::EmbeddingSpaceChanged)
        );
    }
}

#[test]
fn backfill_selection_work_is_linear() {
    let small = measure(512);
    let large = measure(1024);
    assert!(
        large < small * 23 / 10,
        "Selection VM steps should scale linearly: {small} -> {large}"
    );
}

#[test]
fn http_checks_indices_count_dimensions_zero_norm_and_accepts_shuffled_items() {
    let cases = [
        (
            r#"{"data":[{"index":1,"embedding":[0,2]},{"index":0,"embedding":[3,0]}]}"#,
            true,
        ),
        (
            r#"{"data":[{"index":0,"embedding":[1,0]},{"index":0,"embedding":[1,0]}]}"#,
            false,
        ),
        (r#"{"data":[{"index":0,"embedding":[1,0]}]}"#, false),
        (
            r#"{"data":[{"index":2,"embedding":[1,0]},{"index":0,"embedding":[1,0]}]}"#,
            false,
        ),
        (
            r#"{"data":[{"index":0,"embedding":[0,0]},{"index":1,"embedding":[1,0]}]}"#,
            false,
        ),
        (
            r#"{"data":[{"index":0,"embedding":[1]},{"index":1,"embedding":[1,0]}]}"#,
            false,
        ),
    ];
    for (body, valid) in cases {
        let (mut engine, server) = http_reply(body.into(), "", |_| {});
        let response = engine.embed(&["one".into(), "two".into()]);
        assert_eq!(response.is_ok(), valid);
        if valid {
            let values = response.unwrap();
            assert_eq!(values[0].as_ref().unwrap(), &vec![1.0, 0.0]);
        }
        let sent: serde_json::Value = serde_json::from_str(&server.join().unwrap()).unwrap();
        assert_eq!(sent["input"], serde_json::json!(["one", "two"]));
        assert!(sent.get("dimensions").is_none());
    }
}

#[test]
fn http_tokenizer_assets_can_exceed_sixteen_mib_but_remain_bounded() {
    let root = tempfile::tempdir().unwrap();
    let tokenizer = root.path().join("tokenizer.json");
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/http-tokenizer/tokenizer.json"),
        &tokenizer,
    )
    .unwrap();
    let mut bytes = std::fs::read(&tokenizer).unwrap();
    bytes.resize(17 * 1024 * 1024, b' ');
    std::fs::write(&tokenizer, bytes).unwrap();
    let json = serde_json::json!({"backend":"http","model":"m","revision":"r","url":"http://127.0.0.1:9/v1/embeddings","dimensions":2,"tokenizer":tokenizer,"max_tokens":16});
    let load = || Engine::load(serde_json::from_value(json.clone()).unwrap());
    load().unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&tokenizer)
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    assert_eq!(
        load().err().unwrap().code,
        crate::ErrorCode::EmbeddingAssetSize
    );
}

#[test]
fn http_dimensions_opt_in_and_token_preflight_apply_to_query_clones() {
    let tokenizer = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/http-tokenizer/tokenizer.json");
    let (mut engine, server) = http_reply(
        r#"{"data":[{"index":0,"embedding":[1,0]}]}"#.into(),
        "",
        |config| {
            let Config::Http(HttpConfig {
                request_dimensions,
                tokenizer: path,
                max_tokens,
                query_prefix,
                ..
            }) = config;
            *request_dimensions = true;
            *path = Some(tokenizer);
            *max_tokens = Some(16);
            *query_prefix = "query: ".into();
        },
    );
    assert_eq!(
        engine.embed(&["cat ".repeat(17)]).unwrap()[0]
            .as_ref()
            .err()
            .unwrap()
            .code,
        crate::ErrorCode::EmbeddingInputTooLarge
    );
    let mut query_engine = engine.query_engine().unwrap();
    let (_root, mut comb, _) = setup();
    let lease = comb.activate_embeddings(engine.space(), true).unwrap();
    let error = run_query(query_engine.as_mut(), lease.clone(), &"cat ".repeat(15))
        .err()
        .unwrap();
    assert_eq!(error.code, crate::ErrorCode::EmbeddingInputTooLarge);
    assert!(error.message.contains("16 tokens"));
    assert!(run_query(query_engine.as_mut(), lease, "cat").is_ok());
    let sent: serde_json::Value = serde_json::from_str(&server.join().unwrap()).unwrap();
    assert_eq!(sent["dimensions"], 2);
    assert_eq!(sent["input"], serde_json::json!(["query: cat"]));
}

#[test]
fn http_preflight_is_bounded_and_policy_changes_require_explicit_replacement() {
    let root = tempfile::tempdir().unwrap();
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/http-tokenizer/tokenizer.json");
    let tokenizer = root.path().join("tokenizer.json");
    std::fs::copy(fixture, &tokenizer).unwrap();
    let mut json = serde_json::json!({"backend":"http","model":"m","revision":"r","url":"http://127.0.0.1:9/v1/embeddings","dimensions":2,"tokenizer":tokenizer,"max_tokens":16});
    let load = |value: serde_json::Value| Engine::load(serde_json::from_value(value).unwrap());
    let a = load(json.clone()).unwrap();
    let (_root, mut comb, _) = setup();
    comb.activate_embeddings(a.space(), true).unwrap();
    for (key, value) in [
        ("max_tokens", serde_json::json!(32)),
        ("request_dimensions", serde_json::json!(true)),
    ] {
        let mut changed = json.clone();
        changed[key] = value;
        let b = load(changed).unwrap();
        assert_ne!(
            a.space().fingerprint().unwrap(),
            b.space().fingerprint().unwrap()
        );
        assert_eq!(
            comb.activate_embeddings(b.space(), false)
                .err()
                .unwrap()
                .code,
            crate::ErrorCode::EmbeddingReplaceRequired
        );
    }
    let mut bytes = std::fs::read(&tokenizer).unwrap();
    bytes.push(b' ');
    std::fs::write(&tokenizer, bytes).unwrap();
    assert_ne!(
        a.space().fingerprint().unwrap(),
        load(json.clone()).unwrap().space().fingerprint().unwrap()
    );
    json.as_object_mut().unwrap().remove("max_tokens");
    assert_eq!(
        load(json).err().unwrap().code,
        crate::ErrorCode::InvalidEmbeddingConfig
    );
}

#[test]
fn static_http_identity_matches_loaded_engine_without_using_endpoint() {
    let config: HttpConfig = serde_json::from_value(serde_json::json!({
        "model":"test", "revision":"fixed", "url":"http://127.0.0.1:1/v1/embeddings",
        "dimensions":3, "query_prefix":"query: ", "document_prefix":"passage: ",
        "request_dimensions":true
    }))
    .unwrap();
    let identity = Engine::http_identity(config.clone()).unwrap();
    let engine = Engine::load(Config::Http(config)).unwrap();
    assert_eq!(identity, *engine.space());
}
