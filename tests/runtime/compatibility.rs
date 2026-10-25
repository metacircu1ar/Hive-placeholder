//! Plain binaries admit provider ranges and verify actual native branch behavior.
use super::*;
mod corpus;
mod transcripts;

#[test]
fn newer_claude_runs_and_branches_with_checked_native_history() {
    let mut f = Harness::builder(Provider::Claude).build();
    let executable = f.tmp.path().join("claude");
    let original = fs::read_to_string(&executable).unwrap();
    let script = original.replace("\"2.1.287\"", "\"2.9.999\"");
    assert_ne!(original, script);
    fs::write(executable, script).unwrap();
    f.start();
    let source = super::claude::spawn(&f, "reply:ordinary public protocol");
    let before = f.wait(&source, |a| {
        a["state"] == "idle" && !a["last_finished_turn"].is_null()
    });
    let response = f.ok(json!({"op":"spawn", "from_agent":source, "body":"reply:branch"}));
    let child = response["agent_id"].as_str().unwrap();
    let after = f.wait(child, |a| {
        a["state"] == "idle" && a["branch_phase"] == "bound" && !a["last_finished_turn"].is_null()
    });
    assert_ne!(before["thread_id"], after["thread_id"]);
    assert_eq!(
        f.ok(json!({"op":"agents"}))["agents"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(
        f.events(&source)
            .events
            .iter()
            .any(|event| event.kind == "turn_finished")
    );
    f.shutdown();
}

#[test]
fn branch_uses_selected_executable_not_an_older_warm_process() {
    let mut f = Harness::builder(Provider::Codex)
        .runtime(json!({
            "max_providers":2,"max_loaded":8,"sessions_per_codex_process":4,
            "max_active_turns":4,"max_starting":2,"idle_seconds":0,"linger_seconds":3600
        }))
        .build();
    let script = fs::read_to_string(&f.binary).unwrap().replace(
        "append_line(scenario[\"trace\"], method + \"\\n\")",
        "append_line(str(scenario[\"trace\"]) + '-' + pathlib.Path(__file__).name, method + \"\\n\")",
    );
    let mut paths = Vec::new();
    for (name, branching) in [("old-provider", false), ("new-provider", true)] {
        let directory = f.tmp.path().join(name);
        fs::create_dir(&directory).unwrap();
        support::mock_files(&directory);
        fs::copy(
            f.tmp.path().join("mock_common.py"),
            directory.join("mock_common.py"),
        )
        .unwrap();
        let schema_path = directory.join("codex-schema.json");
        let mut schema: Value = serde_json::from_slice(&fs::read(&schema_path).unwrap()).unwrap();
        if !branching {
            schema["oneOf"]
                .as_array_mut()
                .unwrap()
                .retain(|node| node["properties"]["method"]["enum"][0] != "thread/fork");
        }
        fs::write(schema_path, serde_json::to_vec(&schema).unwrap()).unwrap();
        let binary = directory.join(name);
        fs::write(&binary, &script).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        paths.push(binary);
    }
    fs::remove_file(&f.binary).unwrap();
    std::os::unix::fs::symlink(&paths[0], &f.binary).unwrap();
    let trace = f.tmp.path().join("trace");
    f.set_scenario(Provider::Codex, json!({"trace":trace}));
    f.start();
    let response = f.call(
        json!({"op":"spawn","role":"implementor","cwd":f.tmp.path(),"body":"reply:source prefix"}),
    );
    assert!(
        response.get("error").is_none(),
        "{response}; daemon stderr: {}; daemon stdout: {}; diagnostic log: {}",
        fs::read_to_string(f.tmp.path().join("daemon.stderr.log")).unwrap(),
        fs::read_to_string(f.tmp.path().join("daemon.stdout.log")).unwrap(),
        fs::read_to_string(f.root.join("runtime/logs/hive-daemon.jsonl")).unwrap()
    );
    let source = response["agent_id"].as_str().unwrap().to_owned();
    let before = f.wait(&source, |a| {
        a["state"] == "idle" && !a["last_finished_turn"].is_null()
    });
    fs::remove_file(&f.binary).unwrap();
    std::os::unix::fs::symlink(&paths[1], &f.binary).unwrap();
    let response = f.ok(json!({"op":"spawn","from_agent":source,"body":"reply:branch"}));
    let child = response["agent_id"].as_str().unwrap();
    let after = f.wait(child, |a| {
        a["state"] == "idle" && a["branch_phase"] == "bound"
    });
    assert_ne!(before["provider_id"], after["provider_id"]);
    f.shutdown();
    let old = fs::read_to_string(f.tmp.path().join("trace-old-provider")).unwrap();
    let new = fs::read_to_string(f.tmp.path().join("trace-new-provider")).unwrap();
    assert!(!old.lines().any(|method| method == "thread/fork"));
    assert_eq!(
        old.lines().filter(|method| *method == "turn/start").count(),
        1
    );
    assert_eq!(
        new.lines()
            .filter(|method| *method == "thread/fork")
            .count(),
        1
    );
}
