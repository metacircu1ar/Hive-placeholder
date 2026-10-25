//! Services regressions.
use super::*;

#[test]
fn optional_session_settings_and_generated_environment_reset_by_absence() {
    let launcher = crate::launch::test_cache().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut configured = SessionConfig::for_test(
        Uuid::new_v4().to_string(),
        dir.path().canonicalize().unwrap(),
    );
    std::sync::Arc::make_mut(&mut configured.options).values = [
        ("thinking".into(), json!(4096)),
        ("claude.auto_compact_window".into(), json!(200_000)),
        ("claude.max_output_tokens".into(), json!(50000)),
        ("claude.max_mcp_output_tokens".into(), json!(4000)),
    ]
    .into_iter()
    .collect();
    let mut command = Command::new("claude");
    configured.configure(&mut command, &launcher).unwrap();
    let args: Vec<_> = command.get_args().collect();
    let settings: Value = serde_json::from_str(
        args.windows(2)
            .find(|pair| pair[0] == "--settings")
            .unwrap()[1]
            .to_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(settings["alwaysThinkingEnabled"], true);
    assert_eq!(settings["autoCompactWindow"], 200_000);
    let environment: std::collections::BTreeMap<_, _> = command
        .get_envs()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.map(|value| value.to_string_lossy().into_owned()),
            )
        })
        .collect();
    assert_eq!(
        environment["CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING"].as_deref(),
        Some("1")
    );
    assert_eq!(environment["MAX_THINKING_TOKENS"].as_deref(), Some("4096"));
    assert_eq!(
        environment["CLAUDE_CODE_MAX_OUTPUT_TOKENS"].as_deref(),
        Some("50000")
    );
    assert_eq!(
        environment["MAX_MCP_OUTPUT_TOKENS"].as_deref(),
        Some("4000")
    );

    let mut reset = Command::new("claude");
    SessionConfig::for_test(
        Uuid::new_v4().to_string(),
        dir.path().canonicalize().unwrap(),
    )
    .configure(&mut reset, &launcher)
    .unwrap();
    let reset_args: Vec<_> = reset.get_args().collect();
    let reset_settings: Value = serde_json::from_str(
        reset_args
            .windows(2)
            .find(|pair| pair[0] == "--settings")
            .unwrap()[1]
            .to_str()
            .unwrap(),
    )
    .unwrap();
    assert!(reset_settings.get("alwaysThinkingEnabled").is_none());
    assert!(reset_settings.get("autoCompactWindow").is_none());
    assert!(reset.get_envs().all(|(key, _)| {
        !matches!(
            key.to_str(),
            Some(
                "CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING"
                    | "MAX_THINKING_TOKENS"
                    | "CLAUDE_CODE_MAX_OUTPUT_TOKENS"
                    | "MAX_MCP_OUTPUT_TOKENS"
            )
        )
    }));
}
