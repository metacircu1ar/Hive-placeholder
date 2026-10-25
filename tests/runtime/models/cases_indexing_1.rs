//! Indexing regressions.
use super::*;

#[test]
fn model_change_can_correct_a_rejected_unsent_reservation() {
    let mut f = Harness::builder(Provider::Codex)
        .scenario(json!({"catalog_once_model":"unknown"}))
        .build();
    f.start();
    let spawned = f.ok(json!({"op":"spawn","role":"implementor","cwd":f.tmp.path(),"body":"hold","model":"unknown"}));
    let id = spawned["agent_id"].as_str().unwrap();
    let failed = f.wait(id, |a| a["state"] == "failed");
    assert_eq!(failed["error"], "unknown_model");
    assert_eq!(failed["reserved_turn"]["selection"]["model"], "unknown");
    change(&f, id, "gpt-5.6-luna", "medium");
    assert!(f.status(id)["reserved_turn"].is_null());
    assert!(f.ok(json!({"op":"resume","target":id}))["error"].is_null());
    let active = f.wait(id, |a| a["state"] == "running");
    assert_eq!(active["turn_selection"]["model"], "gpt-5.6-luna");
    let messages = f.mailbox(id);
    assert_eq!(messages["messages"].as_array().unwrap().len(), 1);
    f.shutdown();
}

#[test]
fn cold_model_discovery_uses_the_agents_pinned_generation_after_repair() {
    let mut f = Harness::builder(Provider::Codex)
        .scenario(json!({"catalog_models":["gpt-5.6-terra","old-catalog-only"]}))
        .build();
    f.start();
    let id = f.spawn("hold");
    let original = f.wait(&id, |a| a["state"] == "running");
    f.shutdown();
    let state = State::new(f.root.clone()).unwrap();
    let profile = Profile {
        provider: Provider::Codex,
        method: Method::Subscription,
        api_key_env: None,
        api_key_file: None,
    };
    let repair = state.begin_repair("main", &profile).unwrap();
    state.finish_repair(&repair, true, None).unwrap();
    assert_ne!(
        state
            .lookup("main", &profile)
            .unwrap()
            .unwrap()
            .id
            .to_string(),
        original["auth_generation"]
    );
    f.start();
    let changed = change(&f, &id, "old-catalog-only", "medium");
    assert_eq!(changed["desired"]["model"], "old-catalog-only");
    assert_eq!(
        f.status(&id)["auth_generation"],
        original["auth_generation"]
    );
    f.shutdown();
}

#[test]
fn model_change_is_durable_deduplicated_and_waits_for_the_next_turn() {
    let mut f = Harness::new();
    f.start();
    let id = f.spawn("hold");
    let running = f.wait(&id, |a| a["state"] == "running");
    let request = json!({"op":"set_options","target":id,"options":{"model":"gpt-5.6-luna","effort":"medium"}});
    let changed = f.call_id("model-change", request.clone());
    assert!(changed["error"].is_null(), "{changed}");
    assert_eq!(f.call_id("model-change", request.clone()), changed);
    let pending = f.status(&id);
    assert_eq!(pending["model"], "gpt-5.6-luna");
    assert_eq!(pending["turn_selection"]["model"], "gpt-5.6-terra");
    assert_eq!(pending["turn_id"], running["turn_id"]);
    let rejected = f.expect_err(
        json!({"op":"set_options","target":id,"options":{"model":"unknown"}}),
        "invalid_value",
    );
    assert_eq!(rejected["error"]["field"], "/options/model");
    let rejected = f.expect_err(
        json!({"op":"set_options","target":id,"options":{"model":"gpt-5.6-sol","effort":"bad"}}),
        "invalid_value",
    );
    assert_eq!(rejected["error"]["field"], "/options/effort");
    f.ok(json!({"op":"interrupt","target":id}));
    f.wait(&id, |a| a["state"] == "idle");
    f.ok(json!({"op":"send","target":id,"body":"hold"}));
    let next = f.wait(&id, |a| a["state"] == "running");
    assert_eq!(next["thread_id"], running["thread_id"]);
    assert_eq!(next["execution_id"], running["execution_id"]);
    assert_eq!(next["turn_selection"]["model"], "gpt-5.6-luna");
    assert_eq!(next["turn_selection"]["effort"], "medium");
    let requests: Vec<Value> = fs::read_to_string(f.provider_home.join("turn-selections.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(requests[1]["model"], "gpt-5.6-luna");
    assert_eq!(requests[1]["effort"], "medium");
    f.ok(json!({"op":"stop","target":id}));
    f.wait(&id, |a| a["state"] == "stopped");
    f.shutdown();
    f.start();
    assert_eq!(f.call_id("model-change", request), changed);
    let stored = f.status(&id);
    assert_eq!(stored["options_revision"], 1);
    assert_eq!(stored["model"], "gpt-5.6-luna");
    let omitted = f.ok(json!({"op":"set_options","target":id,"options":{"model":"gpt-5.6-sol"}}));
    assert_eq!(omitted["desired"]["effort"], "medium", "{omitted}");
    assert_eq!(f.status(&id)["paused"], true);
    f.shutdown();
}

#[test]
fn codex_session_option_change_after_a_model_change_reaches_the_next_turn() {
    let mut f = Harness::builder(Provider::Codex)
        .scenario(json!({"resume_ignores_selection":true,"persist_turn_selection":false}))
        .build();
    f.start();
    let id = f.spawn("hold");
    let original = f.wait(&id, |a| a["state"] == "running");
    change(&f, &id, "gpt-5.6-luna", "medium");
    f.ok(json!({"op":"interrupt","target":id}));
    f.wait(&id, |a| a["state"] == "idle");
    f.ok(json!({"op":"send","target":id,"body":"hold"}));
    f.wait(&id, |a| {
        a["state"] == "running" && a["turn_selection"]["model"] == "gpt-5.6-luna"
    });
    f.ok(json!({"op":"set_options","target":id,"options":{"codex.tool_output_token_limit":1000}}));
    f.ok(json!({"op":"interrupt","target":id}));
    f.wait(&id, |a| a["state"] == "idle");
    f.ok(json!({"op":"send","target":id,"body":"reply: progress"}));
    let finished = f.settled(&id);
    assert_eq!(finished["thread_id"], original["thread_id"]);
    assert_eq!(finished["session_selection"]["model"], "gpt-5.6-terra");
    assert_eq!(
        finished["session_selection"]["options"]["values"]["codex.tool_output_token_limit"],
        1000
    );
    assert_eq!(
        finished["applied_options_revision"],
        finished["options_revision"]
    );
    let turns: Vec<Value> = fs::read_to_string(f.provider_home.join("turn-selections.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(turns.len(), 3);
    assert_eq!(turns[2]["model"], "gpt-5.6-luna");
    assert_eq!(turns[2]["effort"], "medium");
    f.shutdown();
}

#[test]
fn model_change_during_turn_start_keeps_request_and_audit_snapshot_together() {
    let mut f = Harness::new();
    let gate = f.tmp.path().join("start-gate");
    let _release_on_failure = support::ReleaseFile::new(&gate);
    f.set_scenario(Provider::Codex, json!({"turn_start_gate":gate}));
    f.start();
    let id = f.spawn("hold");
    f.wait(&id, |a| a["turn_selection"]["model"] == "gpt-5.6-terra");
    change(&f, &id, "gpt-5.6-sol", "medium");
    fs::write(gate, "go").unwrap();
    let active = f.wait(&id, |a| a["state"] == "running");
    assert_eq!(active["turn_selection"]["revision"], 0);
    assert_eq!(active["options_revision"], 1);
    let events = f.read(&id);
    let started = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["type"] == "turn_started")
        .unwrap();
    assert_eq!(started["data"]["model"], "gpt-5.6-terra");
    assert_eq!(started["data"]["options_revision"], 0);
    assert_eq!(
        started["data"]["selection_verification"],
        "requested_unverified"
    );
    f.shutdown();
}

#[test]
fn claude_model_change_relaunches_once_and_resumes_the_same_conversation() {
    let mut f = super::super::claude::fixture();
    f.start();
    let id = super::super::claude::spawn(&f, "hold");
    let original = f.wait(&id, |a| a["state"] == "running");
    change(&f, &id, "haiku", "low");
    let pending = f.status(&id);
    assert_eq!(pending["turn_selection"]["model"], "sonnet");
    f.ok(json!({"op":"send","target":id,"body":"after-switch"}));
    f.ok(json!({"op":"interrupt","target":id}));
    super::super::claude::answered(&f, &id, "answer:after-switch");
    let updated = f.wait(&id, |a| {
        a["state"] == "idle" && a["execution_id"] != original["execution_id"]
    });
    assert_eq!(updated["thread_id"], original["thread_id"]);
    assert_eq!(updated["session_selection"]["model"], "haiku");
    assert_eq!(updated["session_selection"]["effort"], "low");
    let launches: Vec<Value> =
        fs::read_to_string(super::super::claude::home(&f).join("mock-launches.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    assert_eq!(launches.len(), 2, "{launches:?}");
    assert_eq!(launches[1]["resume"], true);
    assert_eq!(launches[1]["model"], "claude-haiku-4-5");
    assert!(f.reports().iter().all(|r| r["confirmed"] == true));
    f.shutdown();
}

#[test]
fn model_change_during_claude_relaunch_applies_after_the_reserved_turn() {
    let mut f = super::super::claude::fixture();
    f.start();
    let id = super::super::claude::spawn(&f, "hold");
    let initial = f.wait(&id, |a| a["state"] == "running");
    change(&f, &id, "haiku", "low");
    let home = super::super::claude::home(&f);
    let release = f.tmp.path().join("settings-release");
    let _release_on_failure = support::ReleaseFile::new(&release);
    f.update_scenario(Provider::Claude, |value| {
        value["settings_gate"] = json!(release);
    });
    f.ok(json!({"op":"send","target":id,"body":"hold"}));
    f.ok(json!({"op":"interrupt","target":id}));
    wait_until(
        SETTINGS_TIMEOUT,
        PROVIDER_POLL_INTERVAL,
        "Claude settings gate",
        || home.join("settings-waiting").exists().then_some(()),
    );
    change(&f, &id, "sonnet", "medium");
    fs::write(release, "go").unwrap();
    let reserved = f.wait(&id, |a| {
        a["state"] == "running" && a["execution_id"] != initial["execution_id"]
    });
    assert_eq!(reserved["model"], "sonnet");
    assert_eq!(reserved["options_revision"], 2);
    assert_eq!(reserved["turn_selection"]["model"], "haiku");
    assert_eq!(reserved["turn_selection"]["revision"], 1);
    f.ok(json!({"op":"send","target":id,"body":"after-two-changes"}));
    f.ok(json!({"op":"interrupt","target":id}));
    super::super::claude::answered(&f, &id, "answer:after-two-changes");
    let final_state = f.wait(&id, |a| a["state"] == "idle");
    assert_eq!(final_state["session_selection"]["model"], "sonnet");
    assert_eq!(final_state["session_selection"]["revision"], 2);
    assert_eq!(final_state["thread_id"], initial["thread_id"]);
    let launches = fs::read_to_string(home.join("mock-launches.jsonl")).unwrap();
    assert_eq!(launches.lines().count(), 3);
    f.shutdown();
}
