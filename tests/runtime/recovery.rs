use super::*;

#[test]
fn same_boot_restart_allows_fresh_agents_but_never_replays_uncertain_input() {
    let mut f = Harness::new();
    let pid_file = f.tmp.path().join("survivor.pid");
    let stop_file = f.tmp.path().join("survivor.stop");
    let mut survivor = SurvivorRelease {
        stop: stop_file.clone(),
        pid: pid_file.clone(),
        identity: None,
    };
    f.set_scenario(
        Provider::Codex,
        json!({"linger_eof":true,"pid_file":pid_file,"eof_stop_file":stop_file}),
    );
    f.start();
    let id = f.spawn("hold");
    let before = f.wait(&id, |a| a["state"] == "running");
    let pid = fs::read_to_string(&pid_file).unwrap().parse().unwrap();
    let original = hive::state::ProcessIdentity::read(pid).unwrap().unwrap();
    survivor.identity = Some(original.clone());
    let mut daemon = f.daemon.take().unwrap();
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    let state = State::new(f.root.clone()).unwrap();
    let profile = Profile {
        provider: Provider::Codex,
        method: Method::Subscription,
        api_key_env: None,
        api_key_file: None,
    };
    let binding = state.lookup("main", &profile).unwrap().unwrap();
    assert_eq!(
        state
            .operation(&binding, hive::state::OperationKind::Login)
            .err()
            .unwrap()
            .code(),
        "profile_busy"
    );
    let report = serde_json::to_value(state.recovery_report(&binding).unwrap()).unwrap();
    assert_eq!(report["runtime_usable"], true);
    f.set_scenario(Provider::Codex, json!({}));
    f.start();
    let next = f.spawn("complete");
    f.wait(&next, |a| {
        a["task_outcome"] == "completed" && a["state"] == "stopped"
    });
    assert!(!original.absent().unwrap());
    assert_eq!(
        state
            .operation(&binding, hive::state::OperationKind::Login)
            .err()
            .unwrap()
            .code(),
        "profile_busy"
    );
    fs::write(stop_file, "stop").unwrap();
    assert_eq!(
        f.call(json!({"op":"resume","target":id}))["error"]["code"],
        "recovery_required"
    );
    let old = f.call(json!({"op":"status","target":id}));
    assert_eq!(old["agent"]["thread_id"], before["thread_id"]);
    let db = rusqlite::Connection::open(f.root.join("runtime/runtime.sqlite")).unwrap();
    let delivery: String = db
        .query_row(
            "SELECT state FROM messages WHERE recipient=?1 ORDER BY seq LIMIT 1",
            [&id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(delivery, "uncertain");
    f.shutdown();
    assert_eq!(
        serde_json::to_value(state.recovery_report(&binding).unwrap()).unwrap()["runtime_usable"],
        true
    );
}
#[test]
fn failed_provider_does_not_stop_same_profile_siblings_or_fresh_work() {
    let mut f = Harness::new();
    configure_runtime(
        &f,
        json!({"sessions_per_codex_process":1,"max_providers":4}),
        false,
    );
    f.start();
    let sibling = f.spawn("hold");
    let saved = f.wait(&sibling, |a| a["state"] == "running");
    f.set_scenario(Provider::Codex, json!({"cleanup_refused":true}));
    let bad = f.spawn("idle");
    f.settled(&bad);
    f.ok(json!({"op":"stop","target":bad}));
    f.wait(&bad, |a| a["state"] == "failed");
    let active = f.call(json!({"op":"status","target":sibling}));
    assert_eq!(active["agent"]["state"], "running");
    assert_eq!(active["agent"]["provider_id"], saved["provider_id"]);
    f.set_scenario(Provider::Codex, json!({}));
    let fresh = f.spawn("complete");
    f.wait(&fresh, |a| {
        a["task_outcome"] == "completed" && a["state"] == "stopped"
    });
    assert_eq!(
        f.call(json!({"op":"resume","target":bad}))["error"]["code"],
        "recovery_required"
    );
    f.ok(json!({"op":"interrupt","target":sibling}));
    f.wait(&sibling, |a| a["state"] == "idle");
    f.ok(json!({"op":"send","target":sibling,"body":"complete"}));
    f.wait(&sibling, |a| {
        a["task_outcome"] == "completed" && a["state"] == "stopped"
    });
    f.shutdown();
    f.assert_recovery_required();
}
#[test]
fn shared_clean_conversation_resumes_while_another_session_keeps_provider_alive() {
    let mut f = Harness::new();
    configure_runtime(
        &f,
        json!({"sessions_per_codex_process":4,"max_providers":1,"idle_seconds":600}),
        false,
    );
    f.start();
    let sibling = f.spawn("idle");
    f.settled(&sibling);
    let clean = f.spawn("idle");
    let before = f.settled(&clean);
    f.ok(json!({"op":"stop","target":clean}));
    let stopped = f.wait(&clean, |a| a["state"] == "stopped");
    assert_eq!(stopped["conversation_clean"], true);
    assert!(f.call(json!({"op":"resume","target":clean}))["error"].is_null());
    f.ok(json!({"op":"send","target":clean,"body":"complete"}));
    let after = f.wait(&clean, |a| {
        a["state"] == "stopped" && a["task_outcome"] == "completed"
    });
    assert_eq!(before["thread_id"], after["thread_id"]);
    assert_eq!(before["provider_id"], after["provider_id"]);
    f.ok(json!({"op":"terminate","target":sibling}));
    f.wait(&sibling, |a| a["state"] == "stopped");
    f.shutdown();
}

#[test]
fn finished_providers_release_credentials_without_another_ping_or_acquisition() {
    let mut f = Harness::new();
    f.start();
    let id = f.spawn("complete");
    f.wait(&id, |a| {
        a["task_outcome"] == "completed" && a["state"] == "stopped"
    });
    let state = State::new(f.root.clone()).unwrap();
    let profile = Profile {
        provider: Provider::Codex,
        method: Method::Subscription,
        api_key_env: None,
        api_key_file: None,
    };
    let binding = state.lookup("main", &profile).unwrap().unwrap();
    support::wait::wait_until(
        Duration::from_secs(5),
        Duration::from_millis(10),
        "exited provider releases credentials",
        || match state.operation(&binding, hive::state::OperationKind::Login) {
            Ok(op) => {
                op.finish().unwrap();
                Some(())
            }
            Err(error) => {
                assert_eq!(error.code(), "profile_busy");
                None
            }
        },
    );
    f.shutdown();
}

struct SurvivorRelease {
    stop: PathBuf,
    pid: PathBuf,
    identity: Option<hive::state::ProcessIdentity>,
}
impl Drop for SurvivorRelease {
    fn drop(&mut self) {
        let identity = self.identity.take().or_else(|| {
            fs::read_to_string(&self.pid)
                .ok()
                .and_then(|text| text.parse::<u32>().ok())
                .and_then(|pid| hive::state::ProcessIdentity::read(pid).ok().flatten())
        });
        let _ = fs::write(&self.stop, "stop");
        if let Some(identity) = identity {
            let _ = support::wait::try_wait_until(
                support::wait::STARTUP_TIMEOUT,
                support::wait::POLL_INTERVAL,
                || identity.absent().ok().filter(|absent| *absent),
            );
        }
    }
}
