use super::*;
use crate::cli::input::Failure;

#[test]
fn missing_model_advice_is_specific_to_search_and_the_exact_native_outcome() {
    for error in [
        Error::filtered_vectors_missing(),
        Error::active_space_missing(),
        Error::embed_first(),
        Error::not_found(),
    ] {
        let expected = serde_json::json!({"error": error});
        let output = Failure::remote(error, "search", None).render();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&output.stdout).unwrap(),
            expected
        );
        assert!(!output.stderr.contains("--embedding-config"));
    }
    assert!(
        Failure::remote(Error::query_model_missing(), "stats", None)
            .render()
            .stderr
            .is_empty()
    );
    assert!(
        Failure::from(Error::not_found())
            .exclusive(Some(Exclusive::Init))
            .render()
            .stderr
            .is_empty()
    );
    assert!(
        Failure::from(Error::already_open())
            .exclusive(None)
            .render()
            .stderr
            .is_empty()
    );
}

#[test]
fn cleanup_keeps_primary_receipt_causes_and_human_hint_but_mcp_does_not_print_it() {
    for operation in [Exclusive::Init, Exclusive::Embed, Exclusive::Rebuild] {
        let mut primary = Error::already_open().caused_by(std::io::Error::other("primary detail"));
        primary.request_id = Some("operation-receipt".into());
        let mut cleanup = Error::from(std::io::Error::other("cleanup detail"));
        cleanup.file_committed = true;
        let mut expected = primary.clone();
        expected.file_committed = true;
        let failure = || {
            Failure::from(primary.clone())
                .exclusive(Some(operation))
                .with_cleanup(Err(cleanup.clone()))
        };
        let output = failure().render();
        assert_eq!(output.exit_code, 1);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&output.stdout).unwrap(),
            serde_json::json!({"error": expected})
        );
        for text in [
            "exclusive access",
            "same root",
            "primary detail",
            "cleanup detail",
            operation.command(),
        ] {
            assert!(output.stderr.contains(text), "missing {text}");
        }
        let mcp = failure().render_mcp();
        assert!(mcp.stdout.is_empty());
        assert!(!mcp.stderr.contains("exclusive access"));
        assert!(!mcp.stderr.contains("detail"));
        assert!(!mcp.stderr.contains("operation-receipt"));
    }
    let output = Failure::remote(Error::query_model_missing(), "search", None)
        .with_cleanup(Err(Error::from(std::io::Error::other("cleanup detail"))))
        .render();
    assert!(output.stderr.contains("--embedding-config"));
    assert!(output.stderr.contains("cleanup detail"));
    let mcp = Failure::remote(Error::query_model_missing(), "search", None).render_mcp();
    assert!(!mcp.stderr.contains("--embedding-config"));
}
