//! API bootstrap uses deterministic native transports and never a paid service.
use super::*;
mod fixture;
use fixture::*;
mod boundaries;
mod lifecycle;
mod races;

fn models(f: &Harness) -> std::process::Output {
    support::command(env!("CARGO_BIN_EXE_hivectl"))
        .arg("--config")
        .arg(&f.config)
        .arg("--state-dir")
        .arg(&f.root)
        .arg("--codex-bin")
        .arg(&f.binary)
        .arg("--claude-bin")
        .arg(f.tmp.path().join("claude"))
        .env("OPENAI_API_KEY", "ambient-wrong-key")
        .env("CODEX_API_KEY", "ambient-wrong-key")
        .env("ANTHROPIC_API_KEY", "ambient-wrong-key")
        .env("ANTHROPIC_AUTH_TOKEN", "ambient-wrong-key")
        .args(["models", "--auth", "api"])
        .output()
        .unwrap()
}
fn private(output: &std::process::Output) {
    for secret in [CANARY, ROTATED] {
        assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
        assert!(!String::from_utf8_lossy(&output.stderr).contains(secret));
    }
}

#[test]
fn api_codex_discovery_authenticates_each_process_without_persisting_the_key() {
    let (f, home) = fixture(Provider::Codex, json!({"api_key":true}));
    let trace = f.tmp.path().join("api-trace");
    Harness::write_scenario(
        &home,
        json!({"api_key":true,"trace":trace,"api_requirements":{"requirements":{"allowedLoginMethods":["chatgpt","api"]}}}),
    );
    for _ in 0..2 {
        let output = models(&f);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        private(&output);
    }
    let sequence = fs::read_to_string(trace).unwrap();
    let expected = "initialize\ninitialized\nconfigRequirements/read\nconfig/read\naccount/login/start\naccount/read\nmodel/list\n";
    assert_eq!(sequence, expected.repeat(2));
    assert!(!home.join("auth.json").exists());
    tree_private(&f.root);
}

#[test]
fn api_codex_configuration_conflicts_refuse_before_transmitting_the_key() {
    for patch in [
        json!({"api_requirements":{}}),
        json!({"api_requirements_unsupported":true}),
        json!({"api_requirements":{"requirements":{"cliAuthCredentialsStore":"file"}}}),
        json!({"api_requirements":{"requirements":{"cliAuthCredentialsStore":"keyring"}}}),
        json!({"api_requirements":{"requirements":{"modelProvider":"other"}}}),
        json!({"api_requirements":{"requirements":{"allowedLoginMethods":[]}}}),
        json!({"api_requirements":{"requirements":{"allowedLoginMethods":["chatgpt"]}}}),
        json!({"api_requirements":{"requirements":{"allowedLoginMethods":"api"}}}),
        json!({"api_requirements":{"requirements":{"allowedLoginMethods":["api",false]}}}),
        json!({"api_requirements":{"requirements":{"modelProviders":{"openai":{}}}}}),
        json!({"api_config":{"config":{}}}),
        json!({"api_config":{"config":{"cli_auth_credentials_store":"ephemeral","model_provider":"openai","openai_base_url":"https://invalid.example"}}}),
        json!({"api_config":{"config":{"cli_auth_credentials_store":"ephemeral","model_provider":"openai","forced_login_method":"chatgpt"}}}),
        json!({"api_config":{"config":{"cli_auth_credentials_store":"ephemeral","model_provider":"openai","model_providers":{"openai":{"base_url":"https://invalid.example"}}}}}),
    ] {
        let (f, home) = fixture(Provider::Codex, json!({}));
        let trace = f.tmp.path().join("api-trace");
        let mut scenario = patch;
        scenario["api_key"] = json!(true);
        scenario["trace"] = json!(trace);
        let code = if scenario["api_requirements_unsupported"] == true {
            "provider_request_failed"
        } else {
            "policy_conflict"
        };
        Harness::write_scenario(&home, scenario);
        let output = models(&f);
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(!output.status.success());
        assert_eq!(value["error"]["code"], code, "{value}");
        assert!(
            !fs::read_to_string(trace)
                .unwrap()
                .contains("account/login/start")
        );
        private(&output);
        tree_private(&f.root);
    }
}

#[test]
fn api_codex_rejects_wrong_method_native_rejection_and_unexpected_key_file() {
    for (patch, code) in [
        (json!({"api_account":"chatgpt"}), "authentication_required"),
        (
            json!({"api_login_kind":"chatgpt"}),
            "authentication_required",
        ),
        (json!({"api_login_error":true}), "provider_request_failed"),
        (json!({"api_auth_file":true}), "policy_conflict"),
    ] {
        let (f, home) = fixture(Provider::Codex, json!({}));
        let mut scenario = patch.clone();
        scenario["api_key"] = json!(true);
        Harness::write_scenario(&home, scenario);
        let output = models(&f);
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(!output.status.success());
        assert_eq!(value["error"]["code"], code, "{value}");
        private(&output);
        if patch["api_auth_file"] == true {
            assert_eq!(fs::read_to_string(home.join("auth.json")).unwrap(), CANARY);
            // Deliberately misbehaving provider's file is retained, never silently removed.
        } else {
            tree_private(&f.root);
        }
    }
}

#[test]
fn api_claude_discovery_requires_exact_first_party_environment_source() {
    for (account, succeeds) in [
        (
            json!({"apiProvider":"firstParty","apiKeySource":"ANTHROPIC_API_KEY"}),
            true,
        ),
        (
            json!({"apiProvider":"firstParty","subscriptionType":"Team","apiKeySource":"none"}),
            false,
        ),
        (json!({"apiProvider":"firstParty"}), false),
        (
            json!({"apiProvider":"firstParty","apiKeySource":"apiKeyHelper"}),
            false,
        ),
        (
            json!({"apiProvider":"bedrock","apiKeySource":"ANTHROPIC_API_KEY"}),
            false,
        ),
    ] {
        let (f, _) = fixture(
            Provider::Claude,
            json!({"api_account":account,"api_key":true}),
        );
        let output = models(&f);
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(output.status.success(), succeeds, "{value}");
        if !succeeds {
            assert_eq!(value["error"]["code"], "authentication_required", "{value}");
        }
        private(&output);
        tree_private(&f.root);
    }
}
