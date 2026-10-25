use super::*;
use support::wait::{POLL_INTERVAL, TEST_TIMEOUT, try_wait_until, wait_until};

const MESSAGE_POLL_INTERVAL: Duration = Duration::from_millis(30);
const CATALOG_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_QUIET_WINDOW: Duration = Duration::from_millis(500);
const ORPHAN_SETTLE_WINDOW: Duration = Duration::from_millis(250);

fn grouped(slots: usize) -> (Harness, PathBuf) {
    let f = Harness::builder(Provider::Codex)
        .auth_group("plans", &["main", "second"])
        .auth_group("second-choice", &["second"])
        .runtime(json!({"max_loaded":slots,"max_providers":slots,"max_starting":slots,"max_active_turns":slots}))
        .build();
    f.edit_config(|config| {
        config["roles"]["implementor"]["auth"] = json!("plans");
        config["default_provider"] = json!("codex");
        config["providers"]["codex"] =
            json!({"auth":"plans","model":"gpt-5.6-terra","effort":"medium"});
    });
    let state = State::new(f.root.clone()).unwrap();
    let profile = Profile {
        provider: Provider::Codex,
        method: Method::Subscription,
        api_key_env: None,
        api_key_file: None,
    };
    let binding = state.lookup("second", &profile).unwrap().unwrap();
    (f, binding.directory.join("provider"))
}
fn inputs(home: &std::path::Path) -> Vec<Value> {
    let text = match fs::read_to_string(home.join("inputs.jsonl")) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => panic!("cannot read mock provider inputs: {error}"),
    };
    text.lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
fn completed_message(f: &Harness, id: &str) {
    let mut result = Value::Null;
    let completed = try_wait_until(TEST_TIMEOUT, MESSAGE_POLL_INTERVAL, || {
        result = f.ok(json!({"op":"message_result","message_id":id}));
        (result["delivery_state"] == "completed").then_some(())
    });
    assert!(completed.is_some(), "{result}");
}

#[test]
fn auth_round_trip_starts_a_new_conversation_without_reusing_a_historical_slot() {
    let (mut f, second) = grouped(4);
    configure_runtime(
        &f,
        json!({"max_loaded":16,"max_providers":4,
        "sessions_per_codex_process":4,"max_starting":4,"max_active_turns":4,
        "linger_seconds":3600}),
        false,
    );
    f.start();
    let id = f.spawn("reply: first A");
    let startup = f.mailbox(&id);
    let startup = startup["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["body"] == "reply: first A")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let first = f.wait(&id, |a| {
        a["state"] == "idle"
            && f.mailbox(&id)["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["id"] == startup && m["state"] == "completed")
    });
    f.ok(json!({"op":"set_auth","target":id,"auth":"second"}));
    f.wait(&id, |a| {
        a["auth"] == "second" && a["waiting_reason"].is_null()
    });
    let sent = f.ok(json!({"op":"send","target":id,"body":"reply: B"}));
    completed_message(&f, sent["message_id"].as_str().unwrap());
    assert_eq!(inputs(&second).len(), 1);
    f.ok(json!({"op":"set_auth","target":id,"auth":"main"}));
    f.wait(&id, |a| {
        a["auth"] == "main" && a["waiting_reason"].is_null()
    });
    let sent = f.ok(json!({"op":"send","target":id,"body":"reply: second A"}));
    completed_message(&f, sent["message_id"].as_str().unwrap());
    let last = f.wait(&id, |a| {
        a["state"] == "failed" || (a["state"] == "idle" && inputs(&f.provider_home).len() == 2)
    });
    assert_eq!(last["state"], "idle", "{last}");
    assert_ne!(first["thread_id"], last["thread_id"]);
    assert_ne!(first["provider_id"], last["provider_id"]);
    // A, B and the new A conversation each retain their own lifetime charge.
    assert_eq!(
        f.ok(json!({"op":"ping"}))["runtime_resources"]["resident_session_slots"],
        3
    );
    f.shutdown();
}

#[test]
fn auth_fallback_cutoff_uses_same_agent_new_conversation_and_never_replays_source() {
    let (mut f, second) = grouped(1);
    f.set_scenario(Provider::Codex, json!({"quota_once":true}));
    f.start();
    let id = f.spawn("reply: original task");
    let value = f.wait(&id, |a| {
        a["auth"] == "second" && a["state"] == "idle" && inputs(&second).len() == 1
    });
    assert_eq!(value["auth_group"], "plans");
    assert_eq!(value["task_outcome"], "open");
    assert!(value["handoff"].as_str().unwrap().contains("original task"));
    let source = inputs(&f.provider_home);
    let destination = inputs(&second);
    assert_eq!(source.len(), 1, "{source:?}");
    assert_eq!(destination.len(), 1, "{destination:?}");
    assert_ne!(source[0]["thread"], destination[0]["thread"]);
    assert!(
        destination[0]["body"]
            .as_str()
            .unwrap()
            .contains("previous turn was cut off")
    );
    let messages = f.mailbox(&id);
    let rows = messages["messages"].as_array().unwrap();
    assert_eq!(
        rows.iter().filter(|m| m["kind"] == "continuation").count(),
        1
    );
    assert_eq!(
        rows.iter()
            .find(|m| m["body"] == "reply: original task")
            .unwrap()["state"],
        "failed"
    );
    let events = f.read(&id);
    assert_eq!(
        events["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["type"] == "auth_switched")
            .count(),
        1
    );
    f.shutdown();
}

#[test]
fn auth_fallback_roleless_spawn_catalog_skipping_and_overrides_are_durable() {
    let (mut f, _second) = grouped(2);
    f.set_scenario(Provider::Codex, json!({"catalog_models":["gpt-5.6-sol"]}));
    f.start();
    let args = json!({"op":"spawn","cwd":f.tmp.path(),"body":"reply: no role","sandbox":"read-only","instructions":"Specific instructions","api_fallback":false});
    let accepted = f.call_id("roleless", args.clone());
    assert_eq!(f.call_id("roleless", args), accepted);
    let id = accepted["agent_id"].as_str().unwrap();
    let agent = f.wait(id, |a| a["state"] == "idle");
    assert!(agent["role"].is_null());
    assert_eq!(agent["auth"], "second");
    assert_eq!(agent["effort"], "medium");
    assert_eq!(agent["sandbox"], "read-only");
    assert_eq!(agent["api_fallback"], false);
    let detail = f.ok(json!({"op":"status","target":id,"instructions":true}));
    assert_eq!(detail["agent"]["instructions"], "Specific instructions");
    let filtered = f.ok(json!({"op":"agents","auth":"main"}));
    assert!(filtered["agents"].as_array().unwrap().is_empty());
    let groups = f.ok(json!({"op":"auth_groups"}));
    let plans = &groups["groups"][0];
    assert_eq!(plans["mode"], "ordered");
    assert_eq!(plans["members"][1]["bound_agents"], 1);
    assert!(plans["members"][1].get("weight").is_none(), "{plans}");
    assert_eq!(plans["paid_tail"], json!([]));
    // A catalog-skipped first member makes this an ordered choice, not a sticky one.
    assert_eq!(
        agent["auth_decision"],
        json!({"mode":"ordered","reason":"ordered"})
    );
    f.shutdown();
}

#[test]
fn auth_fallback_manual_switch_waits_for_boundary_and_is_replay_safe() {
    let (mut f, second) = grouped(2);
    f.start();
    let id = f.spawn("hold");
    let initial = f.wait(&id, |a| a["state"] == "running");
    let args = json!({"op":"set_auth","target":id,"auth":"second"});
    let accepted = f.call_id("switch", args.clone());
    assert_eq!(accepted["takes_effect"], "next_turn_boundary", "{accepted}");
    assert_eq!(f.call_id("switch", args), accepted);
    let still = f.status(&id);
    assert_eq!(still["turn_id"], initial["turn_id"]);
    assert_eq!(still["auth"], "main");
    f.ok(json!({"op":"interrupt","target":id}));
    f.wait(&id, |a| {
        a["auth"] == "second" && a["waiting_reason"].is_null()
    });
    let reply = f.ok(json!({"op":"send","target":id,"body":"reply: moved"}));
    completed_message(&f, reply["message_id"].as_str().unwrap());
    assert_eq!(inputs(&second).len(), 1);
    f.shutdown();
}

#[test]
fn auth_fallback_manual_quota_wait_retains_destination_until_confirmation_across_restart() {
    let (mut f, second) = grouped(3);
    let until = hive::state::now() + 3600;
    Harness::write_scenario(&second, json!({"quota_once":true,"quota_until":until}));
    f.start();
    let limited = f.ok(json!({"op":"spawn","role":"implementor","auth":"second","cwd":f.tmp.path(),"body":"limit"}));
    let limited = limited["agent_id"].as_str().unwrap();
    f.wait(limited, |a| {
        a["state"] == "failed" && inputs(&second).len() == 1
    });
    let id = f.spawn("hold");
    f.wait(&id, |a| a["state"] == "running");
    f.ok(json!({"op":"set_auth","target":id,"auth":"second-choice"}));
    f.ok(json!({"op":"interrupt","target":id}));
    let waiting = f.wait(&id, |a| a["waiting_reason"] == "quota");
    assert_eq!(waiting["auth_switch_pending"]["target"], "second-choice");
    assert_eq!(waiting["quota_wait_until"], until);
    let sent = f.ok(json!({"op":"send","target":id,"body":"reply: after reset"}));
    f.shutdown();
    f.start();
    let restored = f.ok(json!({"op":"send","target":limited,"body":"reply: quota usable again"}));
    f.resume_after_wakeup(limited);
    completed_message(&f, restored["message_id"].as_str().unwrap());
    completed_message(&f, sent["message_id"].as_str().unwrap());
    let moved = f.wait(&id, |a| a["state"] == "idle");
    assert_eq!(moved["auth"], "second");
    assert_eq!(moved["auth_group"], "second-choice");
    assert!(hive::state::now() < until);
    assert_eq!(inputs(&second).len(), 3);
    f.shutdown();
}

#[test]
fn auth_fallback_new_manual_choice_supersedes_pending_switch() {
    let (mut f, second) = grouped(2);
    f.start();
    let id = f.spawn("hold");
    f.wait(&id, |a| a["state"] == "running");
    f.ok(json!({"op":"set_auth","target":id,"auth":"second"}));
    let cancelled = f.ok(json!({"op":"set_auth","target":id,"auth":"main"}));
    assert!(cancelled["error"].is_null(), "{cancelled}");
    f.ok(json!({"op":"interrupt","target":id}));
    let sent = f.ok(json!({"op":"send","target":id,"body":"reply: retained"}));
    completed_message(&f, sent["message_id"].as_str().unwrap());
    let agent = f.wait(&id, |a| a["state"] == "idle");
    assert_eq!(agent["auth"], "main");
    assert!(agent["auth_switch_pending"].is_null());
    assert!(agent["waiting_reason"].is_null());
    assert!(inputs(&second).is_empty());
    f.shutdown();
}

#[test]
fn auth_fallback_disconnect_keeps_uncertainty_and_cleanup_fence() {
    let (mut f, second) = grouped(1);
    f.set_scenario(
        Provider::Codex,
        json!({"quota_once":true,"quota_disconnect":true}),
    );
    f.start();
    let id = f.spawn("reply: original uncertain");
    let fenced = f.wait(&id, |a| {
        a["state"] == "failed" && a["recovery_fence"].is_string()
    });
    assert_eq!(fenced["auth"], "main");
    assert!(inputs(&second).is_empty());
    let messages = f.mailbox(&id);
    let rows = messages["messages"].as_array().unwrap();
    assert_eq!(
        rows.iter()
            .find(|m| m["body"] == "reply: original uncertain")
            .unwrap()["state"],
        "uncertain"
    );
    assert_eq!(
        rows.iter().filter(|m| m["kind"] == "continuation").count(),
        1
    );
    assert_eq!(inputs(&f.provider_home).len(), 1);
    f.shutdown();
}

#[test]
fn auth_fallback_claude_limit_handoff_survives_restart() {
    let mut f = Harness::builder(Provider::Claude)
        .auth_group("claude-plans", &["claude", "claude-second"])
        .build();
    f.edit_config(|config| config["roles"]["claude"]["auth"] = json!("claude-plans"));
    let state = State::new(f.root.clone()).unwrap();
    let profile = Profile {
        provider: Provider::Claude,
        method: Method::Subscription,
        api_key_env: None,
        api_key_file: None,
    };
    let first = state.register("claude", &profile).unwrap();
    let second = state.register("claude-second", &profile).unwrap();
    let first = first.directory.join("provider");
    let second = second.directory.join("provider");
    Harness::write_scenario(&first, json!({"quota_once":true}));
    f.start();
    let id = super::claude::spawn(&f, "original Claude task");
    let moved = f.wait(&id, |a| {
        a["auth"] == "claude-second" && a["state"] == "idle" && inputs(&second).len() == 1
    });
    let thread = moved["thread_id"].clone();
    assert!(
        moved["handoff"]
            .as_str()
            .unwrap()
            .contains("original Claude task")
    );
    f.shutdown();
    f.start();
    let sent = f.ok(json!({"op":"send","target":id,"body":"second turn"}));
    completed_message(&f, sent["message_id"].as_str().unwrap());
    let resumed = f.wait(&id, |a| a["state"] == "idle");
    assert_eq!(resumed["thread_id"], thread);
    let launches = fs::read_to_string(second.join("mock-launches.jsonl")).unwrap();
    assert!(launches.lines().all(|line| {
        serde_json::from_str::<Value>(line).unwrap()["instructions"]
            .as_str()
            .unwrap()
            .contains("original Claude task")
    }));
    assert_eq!(inputs(&first).len(), 1);
    f.shutdown();
}

#[test]
fn auth_fallback_known_reset_wakes_same_conversation() {
    let (mut f, _) = grouped(1);
    f.edit_config(|config| {
        config["auth_groups"]["plans"] = json!({"mode":"ordered","members":["main"]});
    });
    f.set_scenario(
        Provider::Codex,
        json!({"quota_once":true,"quota_seconds":4}),
    );
    f.start();
    let id = f.spawn("reply: original");
    let waiting = f.wait(&id, |a| a["waiting_reason"] == "quota");
    let thread = waiting["thread_id"].clone();
    let until = waiting["quota_wait_until"].as_u64().unwrap();
    let resumed = f.wait(&id, |a| {
        a["state"] == "idle" && inputs(&f.provider_home).len() == 2
    });
    assert_eq!(resumed["thread_id"], thread);
    assert!(hive::state::now() >= until);
    assert!(resumed["waiting_reason"].is_null());
    f.shutdown();
}

#[test]
fn auth_fallback_cold_catalog_safely_drains_a_shared_process() {
    use std::io::Write;
    let (mut f, second) = grouped(2);
    f.edit_config(|config| {
        config["runtime"]["max_providers"] = json!(1);
        config["runtime"]["sessions_per_codex_process"] = json!(2);
        config["runtime"]["cohort_min_seconds"] = json!(0);
    });
    f.set_scenario(Provider::Codex, json!({"quota_body":"limit now"}));
    f.start();
    let id = f.spawn("hold");
    f.wait(&id, |a| a["state"] == "running");
    let peer = f.ok(json!({"op":"spawn","role":"pong","cwd":f.tmp.path(),"body":"reply: peer"}));
    let peer = peer["agent_id"].as_str().unwrap();
    let original_peer = f.wait(peer, |a| {
        a["state"] == "idle" && inputs(&f.provider_home).len() == 2
    });
    f.ok(json!({"op":"interrupt","target":id}));
    f.wait(&id, |a| a["turn_id"].is_null());
    // Invalidate both cached catalogs while leaving the shared native process alive.
    fs::OpenOptions::new()
        .append(true)
        .open(&f.binary)
        .unwrap()
        .write_all(b"\n# new binary identity\n")
        .unwrap();
    f.ok(json!({"op":"send","target":id,"body":"limit now"}));
    f.wait(&id, |a| {
        a["auth"] == "second" && a["state"] == "idle" && inputs(&second).len() == 1
    });
    let retained = f.status(peer);
    assert_eq!(retained["auth"], "main");
    assert_eq!(retained["thread_id"], original_peer["thread_id"]);
    assert!(retained["recovery_fence"].is_null());
    f.shutdown();
}

#[test]
fn auth_fallback_startup_reverifies_authentication_hold() {
    let (mut f, _) = grouped(1);
    f.edit_config(|config| {
        config["auth_groups"]["plans"] = json!({"mode":"ordered","members":["main"]});
    });
    f.set_scenario(
        Provider::Codex,
        json!({"quota_once":true,"quota_error":"unauthorized"}),
    );
    f.start();
    let id = f.spawn("reply: recover authentication");
    let waiting = f.wait(&id, |a| a["waiting_reason"] == "quota");
    assert!(waiting["quota_wait_until"].as_u64().unwrap() > hive::state::now());
    let auth = f.ok(json!({"op":"auth_availability","auth":"main"}));
    assert_eq!(auth["availability"][0]["state"], "limited");
    assert_eq!(auth["availability"][0]["reason"], "authentication");
    f.shutdown();
    f.start();
    f.wait(&id, |a| {
        a["state"] == "idle" && inputs(&f.provider_home).len() == 2
    });
    let auth = f.ok(json!({"op":"auth_availability","auth":"main"}));
    assert_eq!(auth["availability"][0]["state"], "available");
    f.shutdown();
}

#[test]
fn auth_fallback_unknown_reset_admits_one_probe_after_restart_then_manual_pin_unblocks() {
    let (mut f, _) = grouped(4);
    f.edit_config(|config| {
        config["auth_groups"]["plans"] = json!({"mode":"ordered","members":["main"]});
    });
    f.set_scenario(
        Provider::Codex,
        json!({"quota_once":true,"quota_until":null}),
    );
    f.start();
    let first = f.spawn("reply: original input");
    f.wait(&first, |a| a["waiting_reason"] == "quota");
    let mut peers = Vec::new();
    for n in 0..8 {
        peers.push(f.spawn(&format!("reply: peer {n}")));
    }
    assert_eq!(inputs(&f.provider_home).len(), 1);
    f.shutdown();
    // Advance only this persisted fixture deadline. Production has no clock override.
    {
        let db = rusqlite::Connection::open(f.root.join("runtime/runtime.sqlite")).unwrap();
        db.execute(
            "UPDATE login_availability SET retry_at=?1 WHERE profile='main'",
            [i64::try_from(hive::state::now()).unwrap() + 2],
        )
        .unwrap();
    }
    fs::remove_file(f.provider_home.join("quota-seen")).unwrap();
    f.start();
    wait_until(TEST_TIMEOUT, MESSAGE_POLL_INTERVAL, "retry probe", || {
        (inputs(&f.provider_home).len() >= 2).then_some(())
    });
    std::thread::sleep(PROBE_QUIET_WINDOW);
    assert_eq!(
        inputs(&f.provider_home).len(),
        2,
        "more than one group probe attempted"
    );
    let held = f.ok(json!({"op":"auth_availability","auth":"main"}));
    assert_eq!(held["availability"][0]["state"], "limited");
    assert!(held["availability"][0]["retry_at"].as_u64().unwrap() > hive::state::now() + 3000);
    // Explicit profile selection bypasses this hold, including on the same binding.
    let selected = f.ok(json!({"op":"set_auth","target":first,"auth":"main"}));
    assert!(selected["error"].is_null(), "{selected}");
    assert!(selected["auth_group"].is_null());
    f.wait(&first, |a| {
        a["state"] == "idle" && a["waiting_reason"].is_null()
    });
    for peer in peers {
        f.wait(&peer, |a| a["state"] == "idle");
    }
    let rows = f.mailbox(&first);
    assert_eq!(
        rows["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["body"] == "reply: original input")
            .unwrap()["state"],
        "failed"
    );
    f.shutdown();
}

#[test]
fn auth_fallback_purging_while_switch_catalog_is_pending_keeps_daemon_alive() {
    let (mut f, second) = grouped(4);
    f.set_scenario(Provider::Codex, json!({"quota_body":"trigger quota"}));
    f.start();
    let id = f.spawn("reply: initial");
    f.settled(&id);
    let release = f.tmp.path().join("release-catalog");
    let waiting = f.tmp.path().join("waiting-catalog");
    Harness::write_scenario(
        &second,
        json!({"model_list_gate":release,"model_list_waiting":waiting}),
    );
    let root = f.root.clone();
    let release_on_failure = release.clone();
    let pending = support::ThreadGuard::new(
        std::thread::spawn(move || {
            runtime::client(
                &root,
                "refresh-second",
                json!({"op":"login_restored","auth":"second"}),
            )
            .unwrap()
        }),
        move || {
            let _ = fs::write(release_on_failure, "release");
        },
    );
    wait_until(
        CATALOG_TIMEOUT,
        POLL_INTERVAL,
        "pending switch catalog",
        || waiting.exists().then_some(()),
    );
    f.ok(json!({"op":"send","target":id,"body":"trigger quota"}));
    f.wait(&id, |a| {
        a["waiting_reason"] == "switch" && a["conversation_clean"] == true && a["turn_id"].is_null()
    });
    f.ok(json!({"op":"terminate","target":id}));
    f.wait(&id, |a| {
        a["state"] == "stopped" && a["task_outcome"] == "cancelled"
    });
    let purged = f.ok(json!({"op":"purge","target":id}));
    assert!(purged["error"].is_null(), "{purged}");
    fs::write(release, "release").unwrap();
    let restored = pending.join().unwrap();
    assert!(restored["error"].is_null(), "{restored}");
    // Exercise subsequent real control and worker activity after the orphaned job settles.
    std::thread::sleep(ORPHAN_SETTLE_WINDOW);
    let next = f
        .ok(json!({"op":"spawn","auth":"second","cwd":f.tmp.path(),"body":"reply: still working"}));
    let next = next["agent_id"].as_str().unwrap();
    f.wait(next, |a| a["state"] == "idle");
    assert_eq!(f.ok(json!({"op":"ping"}))["status"], "running");
    f.shutdown();
}

#[test]
fn auth_fallback_startup_finds_offline_repair_of_spending_held_generation() {
    let (mut f, _) = grouped(2);
    f.edit_config(|config| {
        config["auth_groups"]["plans"] = json!({"mode":"ordered","members":["main"]});
    });
    f.set_scenario(
        Provider::Codex,
        json!({"quota_once":true,"quota_error":"billingError"}),
    );
    f.start();
    let id = f.spawn("reply: offline repair");
    let old = f.wait(&id, |a| a["waiting_reason"] == "quota");
    assert!(old["quota_wait_until"].is_null());
    f.shutdown();
    let state = State::new(f.root.clone()).unwrap();
    let profile = Profile {
        provider: Provider::Codex,
        method: Method::Subscription,
        api_key_env: None,
        api_key_file: None,
    };
    let repair = state.begin_repair("main", &profile).unwrap();
    assert!(state.finish_repair(&repair, true, None).unwrap());
    let generation = repair.binding.id;
    let home = repair.binding.directory.join("provider");
    drop(repair);
    f.start();
    let recovered = f.wait(&id, |a| {
        a["state"] == "idle"
            && a["auth_generation"] == generation.to_string()
            && f.mailbox(&id)["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["kind"] == "continuation" && m["state"] == "completed")
    });
    assert_eq!(recovered["auth"], "main");
    assert_eq!(recovered["auth_group"], "plans");
    assert_eq!(inputs(&f.provider_home).len(), 1);
    assert_eq!(inputs(&home).len(), 1);
    let status = f.ok(json!({"op":"auth_availability","auth":"main"}));
    let prior = status["availability"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["generation"] == old["auth_generation"])
        .unwrap();
    assert_eq!(prior["state"], "unavailable");
    assert_eq!(prior["reason"], "spend_cap");
    f.shutdown();
}
