use super::*;
#[test]
fn environment_is_allowlisted() {
    let entries = [
        ("OPENAI_API_KEY", "canary"),
        ("ANTHROPIC_API_KEY", "canary"),
        ("CODEX_ACCESS_TOKEN", "canary"),
        ("AWS_PROFILE", "canary"),
        ("PATH", "/usr/bin"),
        ("DISPLAY", ":0"),
    ];
    let env = child_environment(
        Provider::Codex,
        Path::new("/private/profile"),
        entries.into_iter().map(|(k, v)| EnvironmentVariable {
            key: k.into(),
            value: v.into(),
        }),
    );
    assert!(env.values().all(|v| v != "canary"));
    assert_eq!(
        env.get(OsStr::new("CODEX_HOME")).unwrap(),
        "/private/profile"
    );
    assert_eq!(env.get(OsStr::new("DISPLAY")).unwrap(), ":0");
}
