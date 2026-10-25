use super::*;

fn isolated_process() -> Harness {
    // These fault injections inspect process exit or reload launch-time scenarios.
    Harness::builder(Provider::Codex)
        .runtime(json!({"sessions_per_codex_process":1}))
        .build()
}

#[test]
fn slow_graceful_shutdown_keeps_daemon_and_profile_reusable() {
    let mut f = Harness::new();
    f.set_scenario(Provider::Codex, json!({"linger_eof":true}));
    f.start();
    let id = f.spawn("idle");
    f.settled(&id);
    f.ok(json!({"op":"stop","target":id}));
    f.wait(&id, |a| a["state"] == "stopped");
    let second = f.spawn("complete");
    f.wait(&second, |a| a["state"] == "stopped");
    assert_eq!(f.call(json!({"op":"ping"}))["status"], "running");
    f.shutdown();
    f.start();
}
#[test]
fn verified_cleanup_then_sigterm_keeps_profile_and_conversation_reusable() {
    let mut f = isolated_process();
    f.set_scenario(Provider::Codex, json!({"require_sigterm":true}));
    f.start();
    let id = f.spawn("idle");
    let before = f.settled(&id);
    f.ok(json!({"op":"stop","target":id}));
    f.wait(&id, |a| a["state"] == "stopped");
    assert_eq!(
        f.reports()[0],
        json!({"method":"sigterm","confirmed":true,"quiescent":true,"reaped":true,"exit_code":0})
    );
    f.set_scenario(Provider::Codex, json!({}));
    f.ok(json!({"op":"resume","target":id}));
    f.ok(json!({"op":"send","target":id,"body":"complete"}));
    let after = f.wait(&id, |a| {
        a["state"] == "stopped" && a["task_outcome"] == "completed"
    });
    assert_eq!(before["thread_id"], after["thread_id"]);
    f.shutdown();
    f.start();
}
#[test]
fn worker_errors_interrupt_active_turn_and_clean_separate_process_group() {
    let mut f = isolated_process();
    let trace = f.tmp.path().join("trace");
    let survivor = f.tmp.path().join("survivor");
    f.set_scenario(Provider::Codex, json!({"trace":trace,"survivor":survivor}));
    f.start();
    let id = f.spawn("input required");
    let failed = f.wait(&id, |a| a["state"] == "failed");
    assert_eq!(failed["error"], "provider_input_required");
    assert_eq!(f.reports()[0]["confirmed"], true);
    let methods = fs::read_to_string(trace).unwrap();
    assert!(methods.ends_with(
        "turn/interrupt\nthread/backgroundTerminals/clean\nthread/backgroundTerminals/list\n"
    ));
    std::thread::sleep(Duration::from_secs(3));
    assert!(!survivor.exists(), "A command escaped provider cleanup");
    assert_eq!(f.call(json!({"op":"ping"}))["status"], "running");
    f.shutdown();
}
#[test]
fn successful_exit_does_not_clear_guards_without_protocol_cleanup() {
    for scenario in [
        json!({"cleanup_refused":true}),
        json!({"missing_turn_ack":true}),
        json!({"wrong_completion":true}),
        json!({"unexpected_turn":true}),
        json!({"malformed_eof":true}),
        json!({"unexpected_eof":true}),
        json!({"reject_turn":true,"rejected_but_started":true}),
    ] {
        let mut f = isolated_process();
        f.set_scenario(Provider::Codex, scenario.clone());
        f.start();
        let id = f.spawn(if scenario["wrong_completion"] == true {
            "hold"
        } else {
            "idle"
        });
        if scenario["missing_turn_ack"] != true && scenario["reject_turn"] != true {
            f.wait(&id, |a| {
                a["state"]
                    == if scenario["wrong_completion"] == true {
                        "running"
                    } else {
                        "idle"
                    }
            });
            f.ok(json!({"op":"stop","target":id}));
        }
        f.wait_timeout(&id, Duration::from_secs(50), |a| a["state"] == "failed");
        assert_eq!(f.call(json!({"op":"ping"}))["status"], "running");
        f.shutdown();
        let report = &f.reports()[0];
        assert_eq!(report["confirmed"], false, "{scenario}: {report}");
        assert_eq!(report["quiescent"], false, "{scenario}: {report}");
        assert_eq!(report["exit_code"], 0, "{scenario}: {report}");
        if scenario["rejected_but_started"] == true {
            let db = rusqlite::Connection::open(f.root.join("runtime/runtime.sqlite")).unwrap();
            let state: String = db
                .query_row(
                    "SELECT state FROM messages WHERE recipient=?1",
                    [&id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(state, "uncertain");
        }
        f.assert_recovery_required();
    }
}
#[test]
fn optional_cursor_and_exiting_terminals_allow_orderly_shutdown() {
    let mut f = isolated_process();
    f.set_scenario(
        Provider::Codex,
        json!({"omit_cursor":true,"exiting_terminal":true}),
    );
    f.start();
    let id = f.spawn("complete");
    f.wait(&id, |a| a["state"] == "stopped");
    assert_eq!(f.reports()[0]["confirmed"], true);
    f.shutdown();
    assert!(!f.root.join("runtime/owner.json").exists());
}
#[test]
fn metadata_timeout_before_thread_creation_does_not_require_reboot() {
    let mut f = Harness::new();
    f.set_scenario(Provider::Codex, json!({"model_list_delay":30.5}));
    f.start();
    let rejected =
        f.call(json!({"op":"spawn","role":"implementor","cwd":f.tmp.path(),"body":"idle"}));
    assert_eq!(rejected["error"]["code"], "provider_timeout", "{rejected}");
    assert!(
        f.call(json!({"op":"agents"}))["agents"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(f.call(json!({"op":"ping"}))["status"], "running");
    f.shutdown();
    assert!(!f.root.join("runtime/owner.json").exists());
    f.set_scenario(Provider::Codex, json!({}));
    f.start();
    let fresh = f.spawn("complete");
    f.wait(&fresh, |a| {
        a["task_outcome"] == "completed" && a["state"] == "stopped"
    });
}
#[test]
fn explicit_provider_rejections_fail_only_the_agent_and_allow_profile_reuse() {
    for scenario in [json!({"reject_turn":true}), json!({"reject_resume":true})] {
        let mut f = isolated_process();
        f.start();
        let id = if scenario["reject_resume"] == true {
            let id = f.spawn("idle");
            f.settled(&id);
            f.ok(json!({"op":"stop","target":id}));
            f.wait(&id, |a| a["state"] == "stopped");
            id
        } else {
            String::new()
        };
        f.set_scenario(Provider::Codex, scenario.clone());
        let id = if id.is_empty() {
            f.spawn("idle")
        } else {
            f.ok(json!({"op":"resume","target":id}));
            id
        };
        let failed = f.wait(&id, |a| a["state"] == "failed");
        assert_eq!(failed["error"], "provider_request_failed");
        if scenario["reject_turn"] == true {
            assert_eq!(
                f.call(json!({"op":"messages","target":id}))["messages"][0]["state"],
                "failed"
            );
        }
        assert!(f.reports().iter().all(|r| r["confirmed"] == true));
        assert_eq!(f.call(json!({"op":"ping"}))["status"], "running");
        f.shutdown();
        assert!(!f.root.join("runtime/owner.json").exists());
        f.start();
    }
}
#[test]
fn rejected_interrupt_waits_for_matching_completion_in_worker_and_cleanup() {
    for body in ["hold", "input required"] {
        let mut f = isolated_process();
        f.set_scenario(
            Provider::Codex,
            json!({"reject_interrupt":true,"survivor":f.tmp.path().join("survivor")}),
        );
        f.start();
        let id = f.spawn(body);
        if body == "hold" {
            f.wait(&id, |a| a["state"] == "running");
            f.ok(json!({"op":"stop","target":id}));
            f.wait(&id, |a| a["state"] == "stopped");
        } else {
            f.wait(&id, |a| a["state"] == "failed");
        }
        assert_eq!(f.reports()[0]["confirmed"], true);
        f.shutdown();
    }
}
