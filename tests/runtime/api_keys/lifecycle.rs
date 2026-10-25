use super::*;

fn source(f: &Harness, name: &str, command: &str) -> std::process::Output {
    support::command(env!("CARGO_BIN_EXE_hivectl"))
        .arg("--config")
        .arg(&f.config)
        .arg("--state-dir")
        .arg(&f.root)
        .arg("--codex-bin")
        .arg(&f.binary)
        .arg("--claude-bin")
        .arg(f.tmp.path().join("claude"))
        .env_remove("HIVE_API_TEST_KEY")
        .args(["auth", command, name])
        .output()
        .unwrap()
}

#[test]
fn api_runtime_restart_and_rotation_resume_the_same_native_conversation() {
    for provider in [Provider::Codex, Provider::Claude] {
        let (mut f, _) = runtime_fixture(provider);
        f.start();
        let id = f.spawn("reply: first");
        let first = startup_finished(&f, &id);
        assert_eq!(first["billing"], "api_key");
        f.ok(json!({"op":"stop","target":id}));
        f.wait(&id, |a| a["state"] == "stopped");
        atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
            .write(f.tmp.path().join("api-key"), ROTATED.as_bytes())
            .unwrap();
        let sent = f.ok(json!({"op":"send","target":id,"body":"reply: second"}));
        f.ok(json!({"op":"resume","target":id}));
        let second = finished(&f, &id, &sent);
        assert_eq!(second["thread_id"], first["thread_id"]);
        assert_ne!(second["provider_id"], first["provider_id"]);
        assert_eq!(second["execution_key"]["revision"], 2);
        f.shutdown();
        f.start();
        let sent = f.ok(json!({"op":"send","target":id,"body":"reply: third"}));
        let third = finished(&f, &id, &sent);
        assert_eq!(third["thread_id"], first["thread_id"]);
        assert_eq!(third["execution_key"]["revision"], 2);
        f.shutdown();
        tree_private(&f.root);
    }
}

#[test]
fn api_source_status_uses_daemon_environment_and_rejects_profile_mismatch() {
    let (mut f, _) = runtime_fixture(Provider::Codex);
    let profile = Profile {
        provider: Provider::Codex,
        method: Method::ApiKey,
        api_key_env: Some("HIVE_API_TEST_KEY".into()),
        api_key_file: None,
    };
    f.edit_config(|config| {
        config["auth_profiles"]["envapi"] = serde_json::to_value(&profile).unwrap();
    });
    let binding = State::new(f.root.clone())
        .unwrap()
        .register("envapi", &profile)
        .unwrap();
    Harness::write_scenario(&binding.directory.join("provider"), json!({"api_key":true}));
    f.start_with_env("HIVE_API_TEST_KEY", CANARY);
    let output = source(&f, "envapi", "status");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(output.status.success(), "{value}");
    assert_eq!(value["environment_source"], "daemon");
    assert_eq!(value["state"], "present_unverified");
    private(&output);
    f.edit_config(|config| {
        config["auth_profiles"]["envapi"]["api_key_env"] = json!("DIFFERENT_KEY");
    });
    let output = source(&f, "envapi", "status");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(!output.status.success());
    assert_eq!(value["error"]["code"], "binding_conflict");
    f.shutdown();
    f.edit_config(|config| {
        config["auth_profiles"]["envapi"]["api_key_env"] = json!("HIVE_API_TEST_KEY");
    });
    let output = source(&f, "envapi", "status");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["environment_source"], "cli");
    assert_eq!(value["state"], "missing");
    tree_private(&f.root);
}

#[test]
fn mixed_group_falls_back_only_after_plan_exhaustion_and_honors_opt_out() {
    // The paid tail keeps the same exhaustion and opt-out rules in both modes.
    for (mode, allowed) in [
        ("ordered", true),
        ("ordered", false),
        ("balanced", true),
        ("balanced", false),
    ] {
        let (mut f, _) = runtime_fixture(Provider::Codex);
        f.edit_config(|config| {
            config["auth_groups"]["mixed"] =
                json!({"mode":mode,"members":["main"],"paid_tail":["api"]});
            config["roles"]["implementor"]["auth"] = json!("mixed");
            config["roles"]["implementor"]["api_fallback"] = json!(allowed);
        });
        f.set_scenario(Provider::Codex, json!({"quota_once":true}));
        f.start();
        let id = f.spawn("reply: original");
        let value = if allowed {
            f.wait(&id, |a| a["auth"] == "api" && a["state"] == "idle")
        } else {
            f.wait(&id, |a| a["waiting_reason"] == "quota")
        };
        assert_eq!(value["auth"], if allowed { "api" } else { "main" });
        let reason = if allowed { "paid_tail" } else { "waiting" };
        assert_eq!(value["auth_decision"], json!({"mode":mode,"reason":reason}));
        if allowed {
            let switched = f
                .events(&id)
                .events
                .into_iter()
                .find(|e| e.kind == "auth_switched")
                .unwrap()
                .data;
            assert_eq!(switched["auth_decision"], value["auth_decision"]);
            let groups = f.ok(json!({"op":"auth_groups"}));
            let mixed = groups["groups"]
                .as_array()
                .unwrap()
                .iter()
                .find(|g| g["name"] == "mixed")
                .unwrap();
            assert_eq!(mixed["mode"], mode);
            let tail = &mixed["paid_tail"][0];
            assert_eq!(
                (&tail["profile"], &tail["method"]),
                (&json!("api"), &json!("api_key"))
            );
            assert_eq!(tail["bound_agents"], 1);
            assert!(tail.get("weight").is_none(), "{mixed}");
            assert_eq!(
                mixed["members"][0].get("weight").is_some(),
                mode == "balanced",
                "{mixed}"
            );
        }
        let messages = f.mailbox(&id);
        let original = messages["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["body"] == "reply: original")
            .unwrap();
        assert_eq!(original["state"], "failed");
        f.shutdown();
        tree_private(&f.root);
    }
}

#[test]
fn api_status_preserves_invalid_key_hold_and_explicit_login_only_grants_unverified_attempt() {
    let (mut f, home) = runtime_fixture(Provider::Codex);
    Harness::write_scenario(
        &home,
        json!({"api_key":true,"quota_once":true,"quota_error":"unauthorized"}),
    );
    f.start();
    let id = f.spawn("reply: rejected");
    f.wait(&id, |a| a["last_finished_turn"]["outcome"] == "failed");
    let output = source(&f, "api", "status");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(output.status.success(), "{value}");
    assert_eq!(value["state"], "present_unverified");
    assert_eq!(value["availability"][0]["state"], "unavailable");
    assert_eq!(value["availability"][0]["reason"], "authentication");
    assert!(value["availability"][0]["retry_at"].is_null());
    let output = source(&f, "api", "login");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(output.status.success(), "{value}");
    assert_eq!(value["environment_source"], "daemon");
    assert_eq!(value["state"], "present_unverified");
    assert_eq!(value["availability"][0]["state"], "available");
    assert!(value["availability"][0]["confirmed_at"].is_null());
    f.shutdown();
    tree_private(&f.root);
    let owner = hive::state::Lock::acquire(&f.root.join("runtime/daemon.lock")).unwrap();
    let output = source(&f, "api", "status");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(!output.status.success());
    assert_eq!(value["error"]["code"], "daemon_busy");
    drop(owner);
}

#[test]
fn failed_subscription_authentication_never_authorizes_paid_fallback() {
    let (mut f, _) = runtime_fixture(Provider::Codex);
    f.edit_config(|config| {
        config["auth_groups"]["mixed"] =
            json!({"mode":"ordered","members":["main"],"paid_tail":["api"]});
        config["roles"]["implementor"]["auth"] = json!("mixed");
    });
    f.set_scenario(
        Provider::Codex,
        json!({"quota_once":true,"quota_error":"unauthorized"}),
    );
    f.start();
    let id = f.spawn("reply: rejected plan");
    let value = f.wait(&id, |a| a["waiting_reason"] == "quota");
    assert_eq!(value["auth"], "main");
    assert_eq!(value["billing"], "subscription");
    f.shutdown();
    tree_private(&f.root);
}
