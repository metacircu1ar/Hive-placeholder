use super::*;
fn report() -> Report {
    Report::new(
        "main",
        &Profile {
            provider: Provider::Codex,
            method: Method::Subscription,
            api_key_env: None,
            api_key_file: None,
        },
        None,
    )
}
#[test]
fn typed_status_never_trusts_arbitrary_output() {
    let mut r = report();
    parse_codex(
        &json!({"result":{"account":{"type":"chatgpt"},"requiresOpenaiAuth":true}}),
        &mut r,
    )
    .unwrap();
    assert_eq!(r.state, Status::Authenticated);
    parse_codex(
        &json!({"result":{"account":{"type":"apiKey"},"requiresOpenaiAuth":true}}),
        &mut r,
    )
    .unwrap();
    assert_eq!(r.state, Status::MethodConflict);
    assert!(
        parse_codex(
            &json!({"result":{"account":{"type":"unknown"},"requiresOpenaiAuth":true}}),
            &mut r
        )
        .is_err()
    );
    assert!(parse_claude(Some(0), b"Logged in", &mut r).is_err());
    parse_claude(
        Some(0),
        br#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty"}"#,
        &mut r,
    )
    .unwrap();
    assert_eq!(r.state, Status::Authenticated);
    assert!(
        parse_claude(
            Some(1),
            br#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty"}"#,
            &mut r
        )
        .is_err()
    );
    parse_claude(
        Some(0),
        br#"{"loggedIn":true,"authMethod":"api_key","apiProvider":"firstParty"}"#,
        &mut r,
    )
    .unwrap();
    assert_eq!(r.state, Status::MethodConflict);
}
#[test]
fn version_metadata_is_bounded_and_not_arbitrary_provider_text() {
    assert!(parse_version(Provider::Codex, b"codex-cli 0.155.0-alpha.2").is_ok());
    assert!(parse_version(Provider::Codex, b"codex-cli secret-canary").is_err());
}

mod probe_cleanup;
