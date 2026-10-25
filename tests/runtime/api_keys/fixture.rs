use super::*;
pub(super) const CANARY: &str = "private-api-bootstrap-canary";
pub(super) const ROTATED: &str = "private-api-rotated-canary";
pub(super) fn fixture(provider: Provider, scenario: Value) -> (Harness, PathBuf) {
    let f = Harness::builder(provider).build();
    let key = f.tmp.path().canonicalize().unwrap().join("api-key");
    fs::write(&key, CANARY).unwrap();
    fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
    let profile = Profile {
        provider,
        method: Method::ApiKey,
        api_key_env: None,
        api_key_file: Some(key),
    };
    f.edit_config(|config| {
        config["auth_profiles"]["api"] = serde_json::to_value(&profile).unwrap();
    });
    let binding = State::new(f.root.clone())
        .unwrap()
        .register("api", &profile)
        .unwrap();
    let home = binding.directory.join("provider");
    Harness::write_scenario(&home, scenario);
    (f, home)
}
pub(super) fn runtime_fixture(provider: Provider) -> (Harness, PathBuf) {
    let (f, home) = fixture(provider, json!({"api_key":true}));
    f.edit_config(|config| {
        config["roles"]["implementor"]["auth"] = json!("api");
        if provider == Provider::Claude {
            config["roles"]["implementor"]["model"] = json!("sonnet");
            config["roles"]["implementor"]["effort"] = json!("high");
        }
        config["runtime"]["max_providers"] = json!(4);
        config["runtime"]["max_loaded"] = json!(8);
        config["runtime"]["max_starting"] = json!(4);
        config["runtime"]["max_active_turns"] = json!(4);
        config["runtime"]["sessions_per_codex_process"] = json!(2);
        config["runtime"]["linger_seconds"] = json!(3600);
    });
    (f, home)
}
pub(super) fn startup_finished(f: &Harness, id: &str) -> Value {
    let page = f.events(id);
    let spawned = page
        .events
        .iter()
        .find(|event| event.agent_id == id && event.kind == "spawned")
        .expect("agent has a spawn event");
    let message = spawned.data["initial_message_id"].as_str().unwrap();
    let agent = f.wait(id, |a| {
        a["state"] == "idle" && a["last_finished_turn"]["message"] == message
    });
    assert_eq!(agent["last_finished_turn"]["outcome"], "completed");
    assert!(f.messages(id).messages.iter().any(|input| {
        input.id == message && input.recipient == id && input.state == "completed"
    }));
    agent
}
pub(super) fn finished(f: &Harness, id: &str, sent: &Value) -> Value {
    let message = sent["message_id"].as_str().unwrap();
    let agent = f.wait(id, |a| {
        a["state"] == "idle" && a["last_finished_turn"]["message"] == message
    });
    assert_eq!(agent["last_finished_turn"]["outcome"], "completed");
    assert_eq!(
        f.ok(json!({"op":"message_result","message_id":message}))["delivery_state"],
        "completed"
    );
    agent
}

pub(super) fn tree_private(path: &std::path::Path) {
    for entry in fs::read_dir(path).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            tree_private(&entry.path());
        } else if entry.file_type().unwrap().is_file() {
            let bytes = fs::read(entry.path()).unwrap();
            for secret in [CANARY, ROTATED] {
                assert!(
                    !bytes
                        .windows(secret.len())
                        .any(|part| part == secret.as_bytes()),
                    "{}",
                    entry.path().display()
                );
            }
        }
    }
}
