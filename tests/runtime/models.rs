use super::*;
use support::wait::{POLL_INTERVAL, STARTUP_TIMEOUT, try_wait_until, wait_until};

const PROVIDER_POLL_INTERVAL: Duration = Duration::from_millis(10);
const CATALOG_CLEANUP_TIMEOUT: Duration = Duration::from_secs(8);
const SETTINGS_TIMEOUT: Duration = Duration::from_secs(10);

fn change(f: &Harness, id: &str, model: &str, effort: &str) -> Value {
    let response =
        f.ok(json!({"op":"set_options","target":id,"options":{"model":model,"effort":effort}}));
    assert!(response["error"].is_null(), "{response}");
    response
}

provider_matrix!(
    catalog_probe_inherits_daemon_ownership_until_its_process_exits,
    catalog_probe_ownership
);

fn catalog_probe_ownership(provider: Provider) {
    let mut f = Harness::builder(provider).build();
    f.start();
    let id = if provider == Provider::Claude {
        super::claude::spawn(&f, "hold")
    } else {
        f.spawn("hold")
    };
    f.wait(&id, |a| a["state"] == "running");
    f.shutdown();
    let release = f.tmp.path().join("catalog-release");
    let pid_file = if provider == Provider::Claude {
        let home = super::claude::home(&f);
        f.update_scenario(Provider::Claude, |value| {
            value["catalog_gate"] = json!(release);
        });
        home.join("catalog-pid")
    } else {
        let path = f.tmp.path().join("catalog-pid");
        f.set_scenario(
            Provider::Codex,
            json!({"model_list_gate":release,"model_list_waiting":path}),
        );
        path
    };
    f.start();
    let root = f.root.clone();
    let release_on_failure = release.clone();
    let pid_on_failure = pid_file.clone();
    let caller = support::ThreadGuard::new(
        std::thread::spawn(move || {
            runtime::client(
                &root,
                "cold-probe",
                json!({"op":"set_options","target":id,"options":{"model":if provider == Provider::Claude { "haiku" } else { "gpt-5.6-sol" }}}),
            )
        }),
        move || release_probe(&release_on_failure, &pid_on_failure),
    );
    let pid = wait_until(
        STARTUP_TIMEOUT,
        PROVIDER_POLL_INTERVAL,
        "catalog probe PID",
        || {
            fs::read_to_string(&pid_file)
                .ok()
                .and_then(|text| text.parse::<u32>().ok())
        },
    );
    let identity = hive::state::ProcessIdentity::read(pid).unwrap().unwrap();
    let mut daemon = f.daemon.take().unwrap();
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    let held = hive::state::Lock::acquire(&f.root.join("runtime/daemon.lock"))
        .err()
        .unwrap();
    assert_eq!(held.code(), "profile_busy", "{held}");
    fs::write(release, "release").unwrap();
    wait_probe_cleanup(&identity, &f.root.join("runtime/daemon.lock"));
    assert!(caller.join().unwrap().is_err());
}

fn release_probe(release: &std::path::Path, pid_file: &std::path::Path) {
    let identity = fs::read_to_string(pid_file)
        .ok()
        .and_then(|text| text.parse::<u32>().ok())
        .and_then(|pid| hive::state::ProcessIdentity::read(pid).ok().flatten());
    let _ = fs::write(release, "release");
    if let Some(identity) = identity {
        let _ = try_wait_until(CATALOG_CLEANUP_TIMEOUT, POLL_INTERVAL, || {
            identity.absent().ok().filter(|absent| *absent)
        });
    }
}

fn wait_probe_cleanup(identity: &hive::state::ProcessIdentity, lock: &std::path::Path) {
    let began = Instant::now();
    wait_until(
        CATALOG_CLEANUP_TIMEOUT,
        POLL_INTERVAL,
        "catalog probe exit",
        || identity.absent().unwrap().then_some(()),
    );
    // Leader absence and release of inherited ownership are separate
    // observations. Require both within the same cleanup deadline.
    let mut last_error = None;
    let released = try_wait_until(
        CATALOG_CLEANUP_TIMEOUT.saturating_sub(began.elapsed()),
        POLL_INTERVAL,
        || match hive::state::Lock::acquire(lock) {
            Ok(_) => Some(()),
            Err(error) => {
                assert_eq!(error.code(), "profile_busy", "{error}");
                last_error = Some(error);
                None
            }
        },
    );
    assert!(released.is_some(), "{last_error:?}");
}

fn check_codex_resume(
    persist: bool,
    model: Option<&str>,
    effort: Option<&str>,
    error: Option<&str>,
) {
    let unexpected = error.is_some();
    let mut f = Harness::builder(Provider::Codex)
        .scenario(
            json!({"resume_ignores_selection":true,"persist_turn_selection":persist,
        "resume_echo_model":model,"resume_echo_effort":effort}),
        )
        .build();
    f.start();
    let id = f.spawn("hold");
    let original = f.wait(&id, |a| a["state"] == "running");
    change(&f, &id, "gpt-5.6-luna", "medium");
    f.ok(json!({"op":"interrupt","target":id}));
    f.wait(&id, |a| a["state"] == "idle");
    f.ok(json!({"op":"send","target":id,"body":"hold"}));
    f.wait(&id, |a| a["state"] == "running");
    f.ok(json!({"op":"stop","target":id}));
    f.wait(&id, |a| a["state"] == "stopped");
    let queued = f.ok(json!({"op":"send","target":id,"body":"hold"}));
    f.ok(json!({"op":"resume","target":id}));
    let restored = f.wait(&id, |a| {
        if unexpected {
            a["state"] == "failed"
        } else {
            a["state"] == "running"
        }
    });
    assert_eq!(restored["thread_id"], original["thread_id"]);
    if unexpected {
        assert_eq!(restored["error"], error.unwrap());
        assert_eq!(restored["session_selection"], original["session_selection"]);
    } else {
        assert_eq!(
            restored["session_selection"]["model"],
            if persist {
                "gpt-5.6-luna"
            } else {
                "gpt-5.6-terra"
            }
        );
        assert_eq!(
            restored["session_selection"]["effort"],
            if persist { "medium" } else { "high" }
        );
        assert_eq!(restored["turn_selection"]["model"], "gpt-5.6-luna");
    }
    let messages = f.mailbox(&id);
    let input = messages["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == queued["message_id"])
        .unwrap();
    assert_eq!(
        input["state"],
        if unexpected { "accepted" } else { "submitted" },
        "{input}"
    );
    assert_eq!(
        fs::read_to_string(f.provider_home.join("turn-selections.jsonl"))
            .unwrap()
            .lines()
            .count(),
        if unexpected { 2 } else { 3 }
    );
    f.shutdown();
}

mod cases_behaviour_1;
mod cases_indexing_1;
mod cases_lifecycle_1;
mod cases_recovery_1;
mod cases_streaming_1;
mod cases_transport_1;
