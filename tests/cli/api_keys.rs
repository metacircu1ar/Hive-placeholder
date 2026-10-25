use super::*;

#[test]
fn file_key_registration_uses_config_directory_and_never_logs_in_natively() {
    let fixture = Fixture::new();
    let key = fixture.tmp.path().join("key");
    fs::write(&key, "private-api-key-canary\n").unwrap();
    fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
    for provider in ["codex", "claude"] {
        fs::write(&fixture.config, format!("auth_profiles: {{api: {{provider: {provider}, method: api_key, api_key_file: key}}}}\n")).unwrap();
        let state = fixture.tmp.path().join(format!("state-{provider}"));
        let run = || {
            support::command(env!("CARGO_BIN_EXE_hivectl"))
                .args([
                    "--config",
                    fixture.config.to_str().unwrap(),
                    "--state-dir",
                    state.to_str().unwrap(),
                    "--codex-bin",
                    fixture.codex.to_str().unwrap(),
                    "--claude-bin",
                    fixture.claude.to_str().unwrap(),
                    "auth",
                    "login",
                    "api",
                ])
                .current_dir("/")
                .output()
                .unwrap()
        };
        let first = run();
        assert!(
            first.status.success(),
            "{}",
            String::from_utf8_lossy(&first.stdout)
        );
        let report: Value = serde_json::from_slice(&first.stdout).unwrap();
        assert_eq!(report["state"], "present_unverified");
        assert_eq!(report["outcome"], "registered");
        assert_eq!(
            report["api_key_file"],
            json!(fixture.tmp.path().canonicalize().unwrap().join("key"))
        );
        let again = run();
        let repeated: Value = serde_json::from_slice(&again.stdout).unwrap();
        assert_eq!(repeated["generation"], report["generation"]);
        assert!(!String::from_utf8_lossy(&first.stdout).contains("private-api-key-canary"));
        let registered = State::new(state).unwrap().generations("api").unwrap();
        assert_eq!(registered.len(), 1);
        assert!(!registered[0].authenticated);
        assert!(registered[0].account_identity.is_none());
        assert!(
            fs::read_dir(registered[0].binding.directory.join("provider"))
                .unwrap()
                .next()
                .is_none()
        );
    }
}
