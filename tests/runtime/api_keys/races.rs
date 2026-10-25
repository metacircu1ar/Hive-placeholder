use super::*;
use support::wait::{POLL_INTERVAL, TEST_TIMEOUT, wait_until};

#[test]
fn rotation_during_catalog_discovery_rejects_stale_proof_before_spawning() {
    let (mut f, home) = runtime_fixture(Provider::Codex);
    let release = f.tmp.path().join("catalog-release");
    let waiting = f.tmp.path().join("catalog-waiting");
    Harness::write_scenario(
        &home,
        json!({"api_key":true,
        "model_list_gate":release,"model_list_waiting":waiting}),
    );
    f.start();
    let root = f.root.clone();
    let cwd = f.tmp.path().to_owned();
    let release_on_failure = release.clone();
    let pending = support::ThreadGuard::new(
        std::thread::spawn(move || {
            runtime::client(
                &root,
                "rotating-spawn",
                json!({"op":"spawn",
            "role":"implementor","cwd":cwd,"body":"reply: guarded"}),
            )
            .unwrap()
        }),
        move || {
            let _ = atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
                .write(&release_on_failure, b"release");
        },
    );
    wait_until(TEST_TIMEOUT, POLL_INTERVAL, "API catalog gate", || {
        waiting.exists().then_some(())
    });
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .write(f.tmp.path().join("api-key"), ROTATED.as_bytes())
        .unwrap();
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .write(&release, b"release")
        .unwrap();
    let refused = pending.join().unwrap();
    assert_eq!(refused["error"]["code"], "agent_busy", "{refused}");
    assert!(
        f.ok(json!({"op":"agents"}))["agents"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let retry = f.ok(json!({"op":"spawn","role":"implementor",
        "cwd":f.tmp.path(),"body":"reply: fresh proof"}));
    let value = startup_finished(&f, retry["agent_id"].as_str().unwrap());
    assert_eq!(value["execution_key"]["revision"], 2);
    f.shutdown();
    tree_private(&f.root);
}

#[test]
fn model_from_rotated_key_catalog_relaunches_before_input_on_the_same_conversation() {
    for restart in [false, true] {
        let (mut f, home) = runtime_fixture(Provider::Codex);
        let old_trace = f.tmp.path().join("old-key-trace");
        let new_trace = f.tmp.path().join("new-key-trace");
        Harness::write_scenario(
            &home,
            json!({"api_key":true,"trace":old_trace,
            "catalog_models":["gpt-5.6-terra"]}),
        );
        f.start();
        let id = f.spawn("reply: original key");
        let before = startup_finished(&f, &id);
        atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
            .write(f.tmp.path().join("api-key"), ROTATED.as_bytes())
            .unwrap();
        Harness::write_scenario(
            &home,
            json!({"api_key":true,"trace":new_trace,
            "catalog_models":["gpt-5.6-terra","new-key-only"]}),
        );
        f.ok(json!({"op":"set_options","target":id,"options":{"model":"new-key-only","effort":"high"}}));
        let changed = f.status(&id);
        assert_eq!(changed["model_key"]["revision"], 2);
        assert_eq!(changed["execution_key"]["revision"], 1);
        if restart {
            f.shutdown();
            f.start();
        }
        let sent = f.ok(json!({"op":"send","target":id,"body":"reply: new-key model"}));
        let after = finished(&f, &id, &sent);
        assert_eq!(after["thread_id"], before["thread_id"]);
        assert_ne!(after["provider_id"], before["provider_id"]);
        assert_eq!(after["model"], "new-key-only");
        assert_eq!(after["execution_key"]["revision"], 2);
        assert_eq!(after["model_key"], after["execution_key"]);
        f.shutdown();
        assert_eq!(
            fs::read_to_string(old_trace)
                .unwrap()
                .lines()
                .filter(|line| *line == "turn/start")
                .count(),
            1
        );
        assert_eq!(
            fs::read_to_string(new_trace)
                .unwrap()
                .lines()
                .filter(|line| *line == "turn/start")
                .count(),
            1
        );
        assert_eq!(
            fs::read_to_string(home.join("inputs.jsonl"))
                .unwrap()
                .lines()
                .count(),
            2
        );
        tree_private(&f.root);
    }
}

#[test]
fn another_rotation_after_model_acceptance_preserves_input_if_model_is_unavailable() {
    let (mut f, home) = runtime_fixture(Provider::Codex);
    Harness::write_scenario(
        &home,
        json!({"api_key":true,"catalog_models":["gpt-5.6-terra"]}),
    );
    f.start();
    let id = f.spawn("reply: original key");
    let before = startup_finished(&f, &id);
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .write(f.tmp.path().join("api-key"), ROTATED.as_bytes())
        .unwrap();
    Harness::write_scenario(
        &home,
        json!({"api_key":true,"catalog_models":["gpt-5.6-terra","new-key-only"]}),
    );
    f.ok(
        json!({"op":"set_options","target":id,"options":{"model":"new-key-only","effort":"high"}}),
    );
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .write(f.tmp.path().join("api-key"), CANARY.as_bytes())
        .unwrap();
    Harness::write_scenario(
        &home,
        json!({"api_key":true,"catalog_models":["gpt-5.6-terra"]}),
    );
    let sent = f.ok(json!({"op":"send","target":id,"body":"reply: retain until supported"}));
    let failed = f.wait(&id, |a| a["state"] == "failed");
    assert_eq!(failed["error"], "unknown_model");
    assert_eq!(failed["execution_key"]["revision"], 3);
    assert_eq!(
        f.ok(json!({"op":"message_result","message_id":sent["message_id"]}))["delivery_state"],
        "accepted"
    );
    assert_eq!(
        fs::read_to_string(home.join("inputs.jsonl"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .write(f.tmp.path().join("api-key"), ROTATED.as_bytes())
        .unwrap();
    Harness::write_scenario(
        &home,
        json!({"api_key":true,"catalog_models":["gpt-5.6-terra","new-key-only"]}),
    );
    f.ok(json!({"op":"resume","target":id}));
    let after = finished(&f, &id, &sent);
    assert_eq!(after["thread_id"], before["thread_id"]);
    assert_eq!(after["execution_key"]["revision"], 4);
    f.shutdown();
    assert_eq!(
        fs::read_to_string(home.join("inputs.jsonl"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    tree_private(&f.root);
}
