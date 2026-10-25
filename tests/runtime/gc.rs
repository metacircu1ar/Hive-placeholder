use super::*;

fn run(f: &Harness, dry: bool) -> (bool, Value) {
    let mut cmd = support::command(env!("CARGO_BIN_EXE_hivectl"));
    cmd.arg("--state-dir")
        .arg(&f.root)
        .args(["gc", "--older-than", "1d"]);
    if dry {
        cmd.arg("--dry-run");
    }
    let output = cmd.output().unwrap();
    (
        output.status.success(),
        serde_json::from_slice(&output.stdout).unwrap(),
    )
}

fn seed(f: &Harness, template: &str, count: usize, outputs: usize) -> Vec<String> {
    let db = rusqlite::Connection::open(f.root.join("runtime/runtime.sqlite")).unwrap();
    let columns = db
        .prepare("PRAGMA table_info(agents)")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .map(Result::unwrap)
        .filter(|c| !["id", "listing_seq"].contains(&c.as_str()))
        .collect::<Vec<_>>();
    let selected = columns
        .iter()
        .map(|c| match c.as_str() {
            "data" => "json_set(data,'$.id',?1)".to_owned(),
            "ended_at" => "1".to_owned(),
            _ => c.clone(),
        })
        .collect::<Vec<_>>()
        .join(",");
    for i in 0..count {
        db.execute(
            &format!(
                "INSERT INTO agents(id,{}) SELECT ?1,{selected} FROM agents WHERE id=?2",
                columns.join(",")
            ),
            rusqlite::params![format!("gc-{i:04}"), template],
        )
        .unwrap();
    }
    let mut ids = vec![];
    for _ in 0..outputs {
        let id = Uuid::new_v4().to_string();
        let file = f.root.join("runtime/output").join(format!("{id}.json"));
        fs::write(&file, "data").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        db.execute(
            "INSERT INTO artifacts(id,bytes,created_at,agent) VALUES(?1,4,0,'gc-0000')",
            [&id],
        )
        .unwrap();
        ids.push(id);
    }
    ids
}

provider_matrix!(
    gc_cli_pages_and_cleanup_preserve_live_peers_with_both_providers,
    gc_cli_pages_and_cleanup_preserve_live_peers_with_both_providers_case
);

fn gc_cli_pages_and_cleanup_preserve_live_peers_with_both_providers_case(provider: Provider) {
    let claude_model = provider == Provider::Claude;
    let mut f = if claude_model {
        claude::fixture()
    } else {
        Harness::new()
    };
    f.start();
    let template = if claude_model {
        claude::spawn(&f, "hold")
    } else {
        f.spawn("hold")
    };
    f.wait(&template, |a| a["state"] == "running");
    f.ok(json!({"op":"terminate","target":template}));
    purge::purgeable(&f, &template);
    f.shutdown();
    let files = seed(&f, &template, 260, 260);
    f.start();
    let peer = if claude_model {
        claude::spawn(&f, "hold")
    } else {
        f.spawn("hold")
    };
    f.wait(&peer, |a| a["state"] == "running");
    let (ok, preview) = run(&f, true);
    assert!(ok, "{preview}");
    assert_eq!(preview["stats"]["eligible_agents"], 260);
    assert_eq!(preview["stats"]["deleted_agents"], 0);
    assert_eq!(preview["selected_preview"].as_array().unwrap().len(), 20);
    assert!(
        f.root
            .join("runtime/output")
            .join(format!("{}.json", files[0]))
            .exists()
    );
    let (ok, result) = run(&f, false);
    assert!(ok, "{result}");
    assert_eq!(result["stats"]["deleted_agents"], 260);
    assert_eq!(result["stats"]["indexed_output_bytes"], 1040);
    assert_eq!(result["acknowledged_pages"], 3);
    assert_eq!(result["complete"], true);
    assert_eq!(
        f.call(json!({"op":"status","target":peer}))["agent"]["state"],
        "running"
    );
    assert!(f.call(json!({"op":"status","target":template}))["agent"].is_object());
    for id in files {
        assert!(
            !f.root
                .join("runtime/output")
                .join(format!("{id}.json"))
                .exists()
        );
    }
    let (ok, empty) = run(&f, false);
    assert!(ok, "{empty}");
    assert_eq!(empty["stats"]["examined_agents"], 0);
    f.ok(json!({"op":"terminate","target":peer}));
    f.wait(&peer, |a| a["state"] == "stopped");
    f.shutdown();
}

#[test]
fn gc_rerun_drains_pending_files_even_after_all_candidates_were_deleted() {
    use std::os::unix::fs::symlink;
    let mut f = Harness::new();
    f.start();
    let template = f.spawn("complete");
    f.wait(&template, |a| {
        a["task_outcome"] == "completed" && a["state"] == "stopped"
    });
    purge::purgeable(&f, &template);
    f.shutdown();
    let ids = seed(&f, &template, 1, 2);
    let unsafe_path = f
        .root
        .join("runtime/output")
        .join(format!("{}.json", ids[0]));
    fs::remove_file(&unsafe_path).unwrap();
    let outside = f.tmp.path().join("keep");
    fs::write(&outside, "retained").unwrap();
    symlink(&outside, &unsafe_path).unwrap();
    f.start();
    let (ok, failed) = run(&f, false);
    assert!(!ok);
    assert_eq!(failed["error"]["code"], "purge_cleanup_pending");
    assert_eq!(failed["gc_progress"]["complete"], false);
    assert_eq!(failed["failed_page"]["request_id"], failed["request_id"]);
    assert_eq!(
        f.call(json!({"op":"status","target":"gc-0000"}))["error"]["code"],
        "agent_purged"
    );
    assert_eq!(fs::read_to_string(outside).unwrap(), "retained");
    fs::remove_file(unsafe_path).unwrap();
    let (ok, retry) = run(&f, false);
    assert!(ok, "{retry}");
    assert_eq!(retry["stats"]["deleted_agents"], 0);
    assert_eq!(retry["complete"], true);
    for id in ids {
        assert!(
            !f.root
                .join("runtime/output")
                .join(format!("{id}.json"))
                .exists()
        );
    }
    let db = rusqlite::Connection::open(f.root.join("runtime/runtime.sqlite")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM artifact_deletions", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    f.shutdown();
}
