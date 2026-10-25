//! Streaming regressions.
use super::*;

#[test]
fn codex_process_options_move_only_at_a_turn_boundary_and_reset_by_omission() {
    let mut f = Harness::new();
    f.start();
    let id = f.spawn("hold");
    let first = f.wait(&id, |a| a["state"] == "running");
    f.ok(json!({"op":"set_options","target":id,"options":{"codex.request_compression":true}}));
    let pending = f.status(&id);
    assert_eq!(pending["provider_id"], first["provider_id"]);
    assert_eq!(pending["turn_id"], first["turn_id"]);
    f.ok(json!({"op":"interrupt","target":id}));
    f.wait(&id, |a| a["state"] == "idle");
    f.ok(json!({"op":"send","target":id,"body":"hold"}));
    let second = f.wait(&id, |a| {
        a["state"] == "running" && a["options_revision"] == a["applied_options_revision"]
    });
    assert_eq!(second["thread_id"], first["thread_id"]);
    assert_ne!(second["provider_id"], first["provider_id"]);
    f.ok(json!({"op":"set_options","target":id,"options":{"codex.request_compression":null}}));
    f.ok(json!({"op":"interrupt","target":id}));
    f.wait(&id, |a| a["state"] == "idle");
    f.ok(json!({"op":"send","target":id,"body":"hold"}));
    let reset = f.wait(&id, |a| {
        a["state"] == "running" && a["options_revision"] == a["applied_options_revision"]
    });
    assert_eq!(reset["thread_id"], first["thread_id"]);
    assert_ne!(reset["provider_id"], second["provider_id"]);
    let launches: Vec<Value> = fs::read_to_string(f.provider_home.join("process-options.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for (state, expected) in [
        (&first, json!([])),
        (&second, json!(["features.enable_request_compression=true"])),
        (&reset, json!([])),
    ] {
        let launch = launches
            .iter()
            .find(|launch| launch["pid"] == state["provider_pid"])
            .unwrap();
        assert_eq!(launch["compression"], expected);
    }
    f.shutdown();
}

#[test]
fn cold_catalog_lookup_returns_busy_when_the_only_provider_slot_is_starting() {
    let mut f = Harness::new();
    f.edit_config(|config| {
        config["runtime"] =
            json!({"max_providers":1,"max_loaded":1,"max_starting":1,"max_active_turns":1});
    });
    f.start();
    let id = f.spawn("hold");
    f.wait(&id, |a| a["state"] == "running");
    f.shutdown();
    let pid = f.tmp.path().join("provider-started");
    f.set_scenario(
        Provider::Codex,
        json!({"model_list_delay":2,"pid_file":pid}),
    );
    f.start();
    f.ok(json!({"op":"send","target":id,"body":"hold"}));
    wait_until(
        STARTUP_TIMEOUT,
        PROVIDER_POLL_INTERVAL,
        "provider startup",
        || pid.exists().then_some(()),
    );
    let response = f.expect_err(
        json!({"op":"set_options","target":id,"options":{"model":"gpt-5.6-luna"}}),
        "agent_busy",
    );
    assert_eq!(response["error"]["code"], "agent_busy", "{response}");
    f.wait(&id, |a| a["state"] == "running");
    // A catalog cache hit needs no extra process, even with that slot occupied.
    change(&f, &id, "gpt-5.6-luna", "medium");
    f.shutdown();
}

#[test]
fn codex_turn_option_is_full_set_then_explicit_reset_then_absent() {
    let mut f = Harness::builder(Provider::Codex)
        .scenario(json!({"provider_version":"0.163.0"}))
        .build();
    f.start();
    let id = f.spawn("hold");
    let mut active = f.wait(&id, |a| a["state"] == "running");

    f.ok(json!({"op":"set_options","target":id,
        "options":{"codex.reasoning_summary":"detailed"}}));
    f.ok(json!({"op":"send","target":id,"body":"hold"}));
    f.ok(json!({"op":"interrupt","target":id}));
    let previous = active["turn_id"].clone();
    active = f.wait(&id, |a| {
        assert_ne!(a["state"], "failed", "{}", f.read(&id));
        a["state"] == "running" && a["turn_id"] != previous
    });

    f.ok(json!({"op":"set_options","target":id,
    "options":{
        "codex.reasoning_summary":null,
        "codex.auto_compact_token_limit":200_000
    }}));
    f.ok(json!({"op":"send","target":id,"body":"hold"}));
    f.ok(json!({"op":"interrupt","target":id}));
    let previous = active["turn_id"].clone();
    active = f.wait(&id, |a| a["state"] == "running" && a["turn_id"] != previous);

    f.ok(json!({"op":"send","target":id,"body":"hold"}));
    f.ok(json!({"op":"interrupt","target":id}));
    let previous = active["turn_id"].clone();
    f.wait(&id, |a| a["state"] == "running" && a["turn_id"] != previous);

    let turns: Vec<Value> = fs::read_to_string(f.provider_home.join("turn-selections.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(turns.len(), 4, "{turns:?}");
    assert_eq!(turns[0]["summary"], Value::Null);
    assert_eq!(turns[1]["summary"], "detailed");
    assert_eq!(turns[2]["summary"], "auto");
    assert_eq!(turns[3]["summary"], Value::Null);
    let sessions: Vec<Value> = fs::read_to_string(f.provider_home.join("thread-selections.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(sessions.len(), 2, "{sessions:?}");
    assert_eq!(sessions[1]["method"], "thread/resume");
    assert_eq!(
        sessions[1]["config"]["model_auto_compact_token_limit"],
        200_000
    );
    f.shutdown();
}
