use super::steering::delivery;
use super::*;
use support::wait::{STARTUP_TIMEOUT, wait_until};

const PROVIDER_POLL_INTERVAL: Duration = Duration::from_millis(10);
const SETTINGS_TIMEOUT: Duration = Duration::from_secs(10);
const CLEANUP_FAILURE_TIMEOUT: Duration = Duration::from_secs(30);
const CLEANUP_MINIMUM_DEADLINE: Duration = Duration::from_secs(20);

fn spawn(f: &Harness, provider: Provider, body: &str) -> String {
    if provider == Provider::Claude {
        super::claude::spawn(f, body)
    } else {
        f.spawn(body)
    }
}
fn initial(f: &Harness, id: &str) -> Value {
    let page = f.mailbox(id);
    json!({"message_id":page["messages"].as_array().unwrap().iter()
        .min_by_key(|m| m["seq"].as_i64().unwrap()).unwrap()["id"]})
}
fn interruptions(f: &Harness, provider: Provider) -> usize {
    fs::read_to_string(f.provider_home(provider).join("interrupts.jsonl"))
        .unwrap()
        .lines()
        .count()
}

provider_matrix!(
    redirect_is_priority_fifo_deduplicated_and_stays_on_the_same_conversation,
    priority_fifo_redirect
);

fn priority_fifo_redirect(provider: Provider) {
    let mut f = Harness::builder(provider).build();
    f.start();
    let id = spawn(&f, provider, "hold");
    let active = f.wait(&id, |a| a["state"] == "running");
    let first = initial(&f, &id);
    let request = json!({"op":"batch","operations":[
        {"op":"send","target":id,"body":"ordinary","request_id":"ordinary"},
        {"op":"send","target":id,"body":"guidance one","mode":"redirect","request_id":"redirect-one"},
        {"op":"send","target":id,"body":"hold","mode":"redirect","request_id":"redirect-two"}
    ]});
    let batch = f.call_id("redirect-batch", request.clone());
    assert!(batch["error"].is_null(), "{batch}");
    let results = &batch["results"];
    delivery(&f, &id, &results[2], "submitted");
    delivery(&f, &id, &results[1], "completed");
    delivery(&f, &id, &results[0], "accepted");
    delivery(&f, &id, &first, "interrupted");
    assert_eq!(results[1]["redirect_target"], results[2]["redirect_target"]);
    assert_eq!(results[1]["redirect_target"]["turn_id"], active["turn_id"]);
    assert_eq!(interruptions(&f, provider), 1);
    assert_eq!(f.call_id("redirect-batch", request.clone()), batch);
    let now = f.status(&id);
    assert_eq!(now["thread_id"], active["thread_id"]);
    assert_ne!(now["turn_id"], active["turn_id"]);
    assert_eq!(interruptions(&f, provider), 1);
    let events = f.read(&id);
    let order: Vec<_> = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == "turn_started")
        .map(|e| e["data"]["message_id"].clone())
        .collect();
    assert_eq!(
        order,
        vec![
            first["message_id"].clone(),
            results[1]["message_id"].clone(),
            results[2]["message_id"].clone()
        ]
    );
    f.shutdown();
    f.start();
    assert_eq!(f.call_id("redirect-batch", request), batch);
    delivery(&f, &id, &results[0], "completed");
    let page = f.mailbox(&id);
    assert_eq!(page["messages"].as_array().unwrap().len(), 4);
    f.shutdown();
}

provider_matrix!(
    redirect_binds_submitting_input_before_its_turn_acknowledgement,
    redirect_before_acknowledgement
);

fn redirect_before_acknowledgement(provider: Provider) {
    let mut f = Harness::builder(provider).build();
    let release = f.tmp.path().join("release-input");
    let _release_on_failure = support::ReleaseFile::new(&release);
    if provider == Provider::Claude {
        f.update_scenario(Provider::Claude, |value| {
            value["input_gate"] = json!(release);
        });
    } else {
        f.set_scenario(Provider::Codex, json!({"turn_start_gate":release}));
    }
    f.start();
    let id = spawn(&f, provider, "hold");
    f.wait(&id, |a| {
        !a["turn_selection"].is_null() && a["turn_id"].is_null()
    });
    let first = initial(&f, &id);
    let request = json!({"op":"send","target":id,"body":"hold","mode":"redirect"});
    let accepted = f.call_id("redirect-submitting", request.clone());
    assert_eq!(
        accepted["redirect_target"]["message_id"],
        first["message_id"]
    );
    assert!(accepted["redirect_target"]["turn_id"].is_null());
    fs::write(release, "go").unwrap();
    delivery(&f, &id, &accepted, "submitted");
    delivery(&f, &id, &first, "interrupted");
    assert_eq!(f.call_id("redirect-submitting", request), accepted);
    assert_eq!(interruptions(&f, provider), 1);
    f.shutdown();
}

provider_matrix!(
    natural_completion_wins_redirect_race_without_changing_its_outcome,
    natural_completion_redirect_race
);

fn natural_completion_redirect_race(provider: Provider) {
    for completing_task in [false, true] {
        let mut f = Harness::builder(provider).build();
        if provider == Provider::Claude {
            f.update_scenario(Provider::Claude, |value| {
                value["complete_before_interrupt"] = json!(true);
            });
        } else {
            f.set_scenario(Provider::Codex, json!({"complete_before_interrupt":true}));
        }
        f.start();
        let id = spawn(&f, provider, "hold");
        let active = f.wait(&id, |a| a["state"] == "running");
        let first = initial(&f, &id);
        if completing_task {
            f.ok(json!({"op":"complete","target":id,"body":"task finished"}));
        }
        let sent = f.ok(json!({"op":"send","target":id,"body":"guidance","mode":"redirect"}));
        delivery(
            &f,
            &id,
            &sent,
            if completing_task {
                "undeliverable"
            } else {
                "completed"
            },
        );
        delivery(&f, &id, &first, "completed");
        assert_eq!(f.status(&id)["thread_id"], active["thread_id"]);
        assert_eq!(interruptions(&f, provider), 1);
        f.shutdown();
    }
}

#[test]
fn redirect_during_claude_relaunch_takes_priority_and_keeps_the_frozen_model() {
    let mut f = Harness::builder(Provider::Claude).build();
    f.start();
    let id = spawn(&f, Provider::Claude, "hold");
    let original = f.wait(&id, |a| a["state"] == "running");
    assert!(
        f.ok(json!({"op":"set_options","target":id,"options":{"model":"haiku","effort":"low"}}))["error"]
            .is_null()
    );
    let release = f.tmp.path().join("settings-release");
    let _release_on_failure = support::ReleaseFile::new(&release);
    let native = f.provider_home(Provider::Claude);
    f.update_scenario(Provider::Claude, |value| {
        value["settings_gate"] = json!(release);
    });
    let ordinary = f.ok(json!({"op":"send","target":id,"body":"ordinary"}));
    f.ok(json!({"op":"interrupt","target":id}));
    wait_until(
        SETTINGS_TIMEOUT,
        PROVIDER_POLL_INTERVAL,
        "Claude settings gate",
        || native.join("settings-waiting").exists().then_some(()),
    );
    assert!(
        f.ok(
            json!({"op":"set_options","target":id,"options":{"model":"sonnet","effort":"medium"}})
        )["error"]
            .is_null()
    );
    let redirected = f.ok(json!({"op":"send","target":id,"body":"hold","mode":"redirect"}));
    assert!(redirected["redirect_target"].is_null());
    fs::write(release, "go").unwrap();
    delivery(&f, &id, &redirected, "submitted");
    delivery(&f, &id, &ordinary, "accepted");
    let active = f.status(&id);
    assert_eq!(active["thread_id"], original["thread_id"]);
    assert_eq!(active["model"], "sonnet");
    assert_eq!(active["turn_selection"]["model"], "haiku");
    assert_eq!(active["turn_selection"]["revision"], 1);
    assert_eq!(
        fs::read_to_string(native.join("mock-launches.jsonl"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    f.ok(json!({"op":"interrupt","target":id}));
    delivery(&f, &id, &ordinary, "completed");
    let finished = f.wait(&id, |a| a["state"] == "idle");
    assert_eq!(finished["last_accepted_selection"]["model"], "sonnet");
    f.shutdown();
}

provider_matrix!(
    redirect_confirms_running_shell_child_gone_before_guidance_completes,
    redirect_cleans_shell_child
);

fn redirect_cleans_shell_child(provider: Provider) {
    let mut f = Harness::builder(provider).build();
    f.start();
    let id = spawn(
        &f,
        provider,
        if provider == Provider::Claude {
            "shell-interrupt"
        } else {
            "shell hold"
        },
    );
    f.wait(&id, |a| a["state"] == "running");
    let pid = if provider == Provider::Claude {
        // Running acknowledges input before the shell hook and helper startup.
        // The fixture emits this receipt only after both child PIDs are ready.
        super::claude::answered(&f, &id, "shell-running");
        fs::read_to_string(f.tmp.path().join("leader.pid"))
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap()
    } else {
        let path = f.provider_home(provider).join("shell-child");
        wait_until(
            STARTUP_TIMEOUT,
            PROVIDER_POLL_INTERVAL,
            "shell child PID",
            || {
                fs::read_to_string(&path)
                    .ok()
                    .and_then(|s| s.trim().parse::<u32>().ok())
            },
        )
    };
    let identity = hive::state::ProcessIdentity::read(pid).unwrap().unwrap();
    let sent = f.ok(json!({"op":"send","target":id,"body":"guidance","mode":"redirect"}));
    delivery(&f, &id, &sent, "completed");
    assert!(identity.absent().unwrap());
    f.shutdown();
}

fn shell_identity(f: &Harness, id: &str) -> hive::state::ProcessIdentity {
    let path = f.provider_home.join(format!("shell-child-{id}"));
    let pid = wait_until(
        STARTUP_TIMEOUT,
        PROVIDER_POLL_INTERVAL,
        "shell child PID",
        || {
            fs::read_to_string(&path)
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
        },
    );
    hive::state::ProcessIdentity::read(pid).unwrap().unwrap()
}

#[test]
fn codex_redirect_cleans_prior_turn_terminals_before_guidance_without_touching_peers() {
    for natural_completion in [false, true] {
        let mut f = Harness::new();
        configure_runtime(
            &f,
            json!({"sessions_per_codex_process":2,"max_providers":1,
            "max_loaded":2,"max_starting":1,"max_active_turns":2,"idle_seconds":600}),
            false,
        );
        f.set_scenario(
            Provider::Codex,
            json!({
                "retain_shell_on_interrupt":true,"complete_before_interrupt":natural_completion
            }),
        );
        f.start();
        let peer = f.spawn("shell hold");
        let peer_active = f.wait(&peer, |a| a["state"] == "running");
        let peer_shell = shell_identity(&f, &peer);
        let id = f.spawn("shell background");
        // Startup also publishes Idle, and the native PID precedes completion.
        // Bind this setup to the initial input before starting the hold turn.
        delivery(&f, &id, &initial(&f, &id), "completed");
        f.wait(&id, |a| a["state"] == "idle");
        let shell = shell_identity(&f, &id);
        assert!(
            !shell.absent().unwrap(),
            "ordinary completion preserves terminals"
        );
        let original = f.ok(json!({"op":"send","target":id,"body":"hold"}));
        delivery(&f, &id, &original, "submitted");
        let active = f.wait(&id, |a| a["state"] == "running");
        assert_eq!(active["provider_id"], peer_active["provider_id"]);
        let guidance = f.ok(json!({"op":"send","target":id,"body":"guidance","mode":"redirect"}));
        assert_eq!(
            guidance["redirect_target"]["message_id"],
            original["message_id"]
        );
        assert_eq!(guidance["redirect_target"]["turn_id"], active["turn_id"]);
        delivery(&f, &id, &guidance, "completed");
        delivery(
            &f,
            &id,
            &original,
            if natural_completion {
                "completed"
            } else {
                "interrupted"
            },
        );
        assert!(shell.absent().unwrap());
        assert!(
            !peer_shell.absent().unwrap(),
            "peer terminal must remain alive"
        );
        assert_eq!(f.status(&peer)["turn_id"], peer_active["turn_id"]);
        let events = f.read(&id);
        let rows = events["events"].as_array().unwrap();
        let cleaned = rows
            .iter()
            .position(|e| e["type"] == "terminal_cleanup_observed")
            .unwrap();
        let started = rows
            .iter()
            .position(|e| {
                e["type"] == "turn_started" && e["data"]["message_id"] == guidance["message_id"]
            })
            .unwrap();
        assert!(cleaned < started);
        assert_eq!(rows[cleaned]["data"]["turn_id"], active["turn_id"]);
        f.shutdown();
    }
}

#[test]
fn codex_cleanup_failure_preserves_outcome_and_does_not_submit_redirect() {
    for natural_completion in [false, true] {
        let mut f = Harness::builder(Provider::Codex)
            .scenario(json!({
                "cleanup_refused":true,"complete_before_interrupt":natural_completion
            }))
            .build();
        f.start();
        let id = f.spawn("hold");
        f.wait(&id, |a| a["state"] == "running");
        let original = initial(&f, &id);
        let guidance = f.ok(json!({"op":"send","target":id,"body":"guidance","mode":"redirect"}));
        f.wait(&id, |a| a["state"] == "failed");
        delivery(
            &f,
            &id,
            &original,
            if natural_completion {
                "completed"
            } else {
                "interrupted"
            },
        );
        delivery(&f, &id, &guidance, "accepted");
        let events = f.read(&id);
        assert!(
            !events["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["type"] == "terminal_cleanup_observed"
                    || (e["type"] == "turn_started"
                        && e["data"]["message_id"] == guidance["message_id"]))
        );
        f.shutdown();
    }
}

#[test]
fn codex_bounded_cleanup_recleans_late_children_and_drains_bursts_before_guidance() {
    let mut f = Harness::builder(Provider::Codex)
        .scenario(json!({
            "late_immediate":true,"late_after_list":20,"late_burst":10000,"exiting_terminal":true
        }))
        .build();
    f.start();
    let id = f.spawn("hold");
    f.wait(&id, |a| a["state"] == "running");
    let sent = f.ok(json!({"op":"send","target":id,"body":"guidance","mode":"redirect"}));
    delivery(&f, &id, &sent, "completed");
    let late = fs::read_to_string(f.provider_home.join(format!("late-shell-{id}"))).unwrap();
    assert!(
        hive::state::ProcessIdentity::read(late.parse().unwrap())
            .unwrap()
            .is_none()
    );
    let events = f.read(&id);
    let observed = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["type"] == "terminal_cleanup_observed")
        .unwrap();
    let evidence = &observed["data"]["observation"];
    assert_eq!(evidence["kind"], "observed_quiet");
    assert_eq!(evidence["late_items_tolerated"], 10002);
    assert!(evidence["quiet_ms"].as_u64().unwrap() >= 3000);
    assert!(evidence["elapsed_ms"].as_u64().unwrap() >= 4800);
    assert!(evidence["recleans"].as_u64().unwrap() >= 1);
    assert_eq!(interruptions(&f, Provider::Codex), 1);
    f.shutdown();
}

#[test]
fn codex_bounded_cleanup_finishes_before_stop_terminate_and_daemon_shutdown() {
    for op in ["stop", "terminate", "shutdown"] {
        let mut f = Harness::new();
        let marker = f.tmp.path().join("first-list");
        f.set_scenario(
            Provider::Codex,
            json!({
                "list_marker":marker,"late_after_list":4
            }),
        );
        f.start();
        let id = f.spawn("hold");
        f.wait(&id, |a| a["state"] == "running");
        let sent = f.ok(json!({"op":"send","target":id,"body":"guidance","mode":"redirect"}));
        wait_until(
            STARTUP_TIMEOUT,
            PROVIDER_POLL_INTERVAL,
            "first cleanup list",
            || marker.exists().then_some(()),
        );
        if op == "shutdown" {
            f.shutdown();
            let late =
                fs::read_to_string(f.provider_home.join(format!("late-shell-{id}"))).unwrap();
            assert!(
                hive::state::ProcessIdentity::read(late.parse().unwrap())
                    .unwrap()
                    .is_none()
            );
            let db = rusqlite::Connection::open(f.root.join("runtime/runtime.sqlite")).unwrap();
            let observed: String = db
                .query_row(
                    "SELECT data FROM events WHERE agent=? AND kind='terminal_cleanup_observed'",
                    [&id],
                    |r| r.get(0),
                )
                .unwrap();
            let observed: Value = serde_json::from_str(&observed).unwrap();
            assert!(observed["observation"]["quiet_ms"].as_u64().unwrap() >= 3000);
            assert_eq!(interruptions(&f, Provider::Codex), 1);
            continue;
        }
        f.ok(json!({"op":op,"target":id}));
        f.wait(&id, |a| a["state"] == "stopped");
        assert_eq!(interruptions(&f, Provider::Codex), 1);
        delivery(
            &f,
            &id,
            &sent,
            if op == "terminate" {
                "undeliverable"
            } else {
                "accepted"
            },
        );
        let events = f.read(&id);
        assert!(
            events["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["type"] == "terminal_cleanup_observed"
                    && e["data"]["observation"]["quiet_ms"].as_u64().unwrap() >= 3000)
        );
        assert!(
            !events["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["type"] == "session_quarantined")
        );
        f.shutdown();
    }
}

#[test]
fn codex_bounded_cleanup_times_out_without_replaying_or_admitting_guidance() {
    let mut f = Harness::builder(Provider::Codex)
        .scenario(json!({"late_continuous":true}))
        .build();
    f.start();
    let id = f.spawn("hold");
    f.wait(&id, |a| a["state"] == "running");
    let first = initial(&f, &id);
    let start = Instant::now();
    let sent = f.ok(json!({"op":"send","target":id,"body":"guidance","mode":"redirect"}));
    f.wait_timeout(&id, CLEANUP_FAILURE_TIMEOUT, |a| a["state"] == "failed");
    assert!(start.elapsed() >= CLEANUP_MINIMUM_DEADLINE);
    delivery(&f, &id, &first, "interrupted");
    delivery(&f, &id, &sent, "accepted");
    let events = f.read(&id);
    assert!(
        !events["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "terminal_cleanup_observed")
    );
    f.shutdown();
}
