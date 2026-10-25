use super::*;

pub(super) fn purgeable(f: &Harness, id: &str) {
    support::wait::wait_until(
        support::wait::TEST_TIMEOUT,
        support::wait::POLL_INTERVAL,
        "purgeable agent",
        || {
            let preview = f.ok(json!({"op":"purge","target":id,"dry_run":true}));
            (preview["blocked_agents"] == 0).then_some(())
        },
    );
}

provider_matrix!(
    purge_cli_works_for_both_providers_and_keeps_native_state_and_working_directory,
    purge_cli_works_for_both_providers_and_keeps_native_state_and_working_directory_case
);

fn purge_cli_works_for_both_providers_and_keeps_native_state_and_working_directory_case(
    provider: Provider,
) {
    let claude_model = provider == Provider::Claude;
    let mut f = if claude_model {
        claude::fixture()
    } else {
        Harness::new()
    };
    f.start();
    let id = if claude_model {
        claude::spawn(&f, "hold")
    } else {
        f.spawn("hold")
    };
    f.wait(&id, |a| a["state"] == "running");
    f.ok(json!({"op":"terminate","target":id}));
    f.wait(&id, |a| a["state"] == "stopped");
    purgeable(&f, &id);
    let provider = if claude_model {
        Provider::Claude
    } else {
        Provider::Codex
    };
    let sentinel = f.provider_home(provider).join("retained-native-history");
    fs::write(&sentinel, "retained").unwrap();
    let run = |dry: bool| {
        let mut cmd = support::command(env!("CARGO_BIN_EXE_hivectl"));
        cmd.arg("--state-dir")
            .arg(&f.root)
            .args(["purge", &id, "--request-id", "cli-purge"]);
        if dry {
            cmd.arg("--dry-run");
        }
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    let preview = run(true);
    assert_eq!(preview["selected_agents"], 1);
    assert_eq!(preview["blocked_agents"], 0);
    let result = run(false);
    assert_eq!(result["counts"]["agents"], 1);
    assert_eq!(result["native_history"], "retained");
    assert_eq!(result["request_id"], "cli-purge");
    assert!(sentinel.exists());
    assert!(f.tmp.path().join("role.md").exists());
    for op in ["status", "read", "messages", "terminate"] {
        f.expect_err(json!({"op":op,"target":id}), "agent_purged");
    }
    f.shutdown();
    f.start();
    let retry = f.call_id(
        "cli-purge",
        json!({"op":"purge","target":id,"recursive":false,"dry_run":false}),
    );
    assert_eq!(retry, result);
    assert_eq!(
        f.call(json!({"op":"purge","target":id}))["error"]["code"],
        "agent_purged"
    );
    f.shutdown();
}

#[test]
fn purge_released_shared_member_preserves_peer_and_later_provider_shutdown() {
    let mut f = Harness::new();
    configure_runtime(
        &f,
        json!({"max_loaded":2,"max_providers":1,"sessions_per_codex_process":2,"max_starting":2,"max_active_turns":2,"idle_seconds":600}),
        false,
    );
    f.start();
    let removed = f.spawn("hold");
    let peer = f.spawn("hold");
    let a = f.wait(&removed, |a| a["state"] == "running");
    let b = f.wait(&peer, |a| a["state"] == "running");
    assert_eq!(a["provider_id"], b["provider_id"]);
    f.ok(json!({"op":"terminate","target":removed}));
    f.wait(&removed, |a| a["state"] == "stopped");
    purgeable(&f, &removed);
    let purged = f.call(json!({"op":"purge","target":removed}));
    assert_eq!(purged["counts"]["agents"], 1, "{purged}");
    assert_eq!(
        f.call(json!({"op":"status","target":peer}))["agent"]["state"],
        "running"
    );
    f.ok(json!({"op":"terminate","target":peer}));
    f.wait(&peer, |a| a["state"] == "stopped");
    let events = f.read(&peer);
    assert!(
        events["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "provider_shutdown" && e["data"]["confirmed"] == true),
        "{events}"
    );
    f.shutdown();
    let db = rusqlite::Connection::open(f.root.join("runtime/runtime.sqlite")).unwrap();
    let provider = b["provider_id"].as_str().unwrap();
    let shutdown: String = db
        .query_row(
            "SELECT data FROM provider_events WHERE provider=?1 AND kind='shutdown'",
            [provider],
            |r| r.get(0),
        )
        .unwrap();
    let shutdown: Value = serde_json::from_str(&shutdown).unwrap();
    assert_eq!(shutdown["shutdown"]["confirmed"], true);
}

#[test]
fn purge_cleanup_failure_returns_retry_id_and_recovers_with_or_without_restart() {
    use std::os::unix::fs::symlink;
    for restart in [false, true] {
        let mut f = Harness::new();
        f.start();
        let id = f.spawn("complete");
        f.wait(&id, |a| {
            a["state"] == "stopped" && a["task_outcome"] == "completed"
        });
        purgeable(&f, &id);
        f.shutdown();
        let dir = f.root.join("runtime/output");
        let first = "00000000-0000-4000-8000-000000000001";
        let second = "00000000-0000-4000-8000-000000000002";
        let file = dir.join(format!("{first}.json"));
        fs::write(&file, "one\n").unwrap();
        fs::set_permissions(file, fs::Permissions::from_mode(0o600)).unwrap();
        let outside = f.tmp.path().join("retained");
        fs::write(&outside, "untouched").unwrap();
        let unsafe_file = dir.join(format!("{second}.json"));
        symlink(&outside, &unsafe_file).unwrap();
        let manifest = f.root.join("runtime/output-manifest.jsonl");
        fs::write(&manifest, "derived manifest\n").unwrap();
        fs::set_permissions(&manifest, fs::Permissions::from_mode(0o600)).unwrap();
        {
            let db = rusqlite::Connection::open(f.root.join("runtime/runtime.sqlite")).unwrap();
            for artifact in [first, second] {
                db.execute(
                    "INSERT INTO artifacts(id,bytes,created_at,agent) VALUES(?1,4,0,?2)",
                    [artifact, &id],
                )
                .unwrap();
            }
        }
        f.start();
        let args = json!({"op":"purge","target":id});
        let failed = f.call_id("retry-cleanup", args.clone());
        assert_eq!(failed["error"]["code"], "purge_cleanup_pending", "{failed}");
        assert_eq!(failed["request_id"], "retry-cleanup");
        assert_eq!(
            f.call(json!({"op":"status","target":id}))["error"]["code"],
            "agent_purged"
        );
        assert!(!dir.join(format!("{first}.json")).exists());
        assert_eq!(fs::read_to_string(&outside).unwrap(), "untouched");
        fs::remove_file(&unsafe_file).unwrap();
        if restart {
            f.shutdown();
            f.start();
        }
        let retried = f.call_id("retry-cleanup", args);
        assert_eq!(retried["counts"]["artifacts"], 2, "{retried}");
        assert!(!manifest.exists());
        f.shutdown();
        let db = rusqlite::Connection::open(f.root.join("runtime/runtime.sqlite")).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM artifact_deletions", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}
