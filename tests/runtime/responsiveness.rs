//! Manual mock-only load probe; report measurements rather than a tight CI timing gate.
use super::*;
use sha2::{Digest, Sha256};

#[test]
#[ignore = "manual release performance probe, run alone with --ignored --exact --nocapture"]
fn control_and_sigterm_under_completion_load() {
    let binary = std::env::var_os("HIVE_RESPONSIVENESS_BINARY").map_or_else(
        || PathBuf::from(env!("CARGO_BIN_EXE_hivectl")),
        PathBuf::from,
    );
    assert_eq!(binary.parent().unwrap().file_name().unwrap(), "release");
    let mut f = Harness::new();
    configure_runtime(
        &f,
        json!({"max_agents":1024,"max_loaded":64,"max_providers":8,
            "sessions_per_codex_process":1,"max_starting":16,"max_active_turns":32}),
        false,
    );
    f.start_binary(&binary, true);
    for batch in 0..8 {
        let operations: Vec<_> = (0..64)
            .map(|i| {
                json!({"op":"spawn",
            "role":"implementor","cwd":f.tmp.path(),"body":"complete",
            "request_id":format!("completion-{batch}-{i}")})
            })
            .collect();
        let result = f.call(json!({"op":"batch","operations":operations}));
        assert!(result.get("error").is_none(), "{result}");
    }
    let load = measure_control_load(&f);
    let daemon_pid = f.daemon.as_ref().unwrap().id();
    // The parent owns this unreaped Child until after delivery. Its PID cannot
    // be reused even if the daemon exits just before kill; no saved PID lookup.
    let signal = support::command(support::python()).args(["-c", concat!(
        "import json,os,signal,sys,time\n",
        "before=time.monotonic_ns()\n",
        "os.kill(int(sys.argv[1]),signal.SIGTERM)\n",
        "after=time.monotonic_ns()\n",
        "print(json.dumps(dict(before=before,after=after,clock=time.get_clock_info('monotonic').implementation)))"
    ), &daemon_pid.to_string()]).output().unwrap();
    assert!(signal.status.success(), "{:?}", signal.stderr);
    let signal: Value = serde_json::from_slice(&signal.stdout).unwrap();
    let began = Instant::now();
    let status = support::wait::wait_until(
        support::wait::TEST_TIMEOUT,
        Duration::from_millis(10),
        "daemon exits after SIGTERM",
        || f.daemon.as_mut().unwrap().try_wait().unwrap(),
    );
    assert!(status.success());
    let exit_wait_seconds = began.elapsed().as_secs_f64();
    f.daemon = None;
    assert!(!f.root.join("runtime/owner.json").exists());
    let reports = f.reports();
    assert!(!reports.is_empty());
    for report in &reports {
        for field in ["confirmed", "quiescent", "reaped"] {
            assert_eq!(report[field], true, "{report}");
        }
    }
    let (raw, shutdown_bounds) = shutdown_diagnostics(&f, daemon_pid, &signal);
    let result = json!({"binary":binary,"binary_sha256":format!("{:x}",Sha256::digest(fs::read(&binary).unwrap())),
        "agents":512,"completed_before_controls":load.completed_before_controls,
        "completed_before_signal":load.completed,"claimed_before_signal":load.claimed,
        "control_socket_roundtrip_seconds":load.control_seconds,"signal":signal,
        "signal_to_shutdown_start_ns_bounds":shutdown_bounds,
        "exit_wait_after_signal_helper_seconds":exit_wait_seconds,
        "shutdown_reports":reports});
    export_probe(&f, &result, &raw);
    println!("{result}");
}

fn shutdown_diagnostics(f: &Harness, daemon_pid: u32, signal: &Value) -> (String, [u64; 2]) {
    let bytes = fs::read(f.root.join("runtime/logs/hive-daemon.jsonl")).unwrap();
    let complete = atomic_fs::read_complete_lines(&bytes).unwrap();
    assert!(complete.incomplete_tail.is_empty());
    let rows: Vec<Value> = complete
        .lines
        .iter()
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    let noticed = rows
        .iter()
        .find(|row| row["event"] == "runtime.timing" && row["phase"] == "signal_shutdown")
        .unwrap();
    assert_eq!(
        rows.iter()
            .find(|r| r["event"] == "daemon.starting")
            .unwrap()["pid"],
        daemon_pid
    );
    let clock = signal["clock"].as_str().unwrap();
    if cfg!(target_os = "macos") {
        assert!(clock.contains("mach_absolute_time"));
    } else {
        assert!(clock.contains("CLOCK_MONOTONIC"));
    }
    let start = noticed["start_ns"].as_u64().unwrap();
    let end = noticed["end_ns"].as_u64().unwrap();
    assert!(start >= signal["before"].as_u64().unwrap());
    assert!(end >= start);
    let lower = start.saturating_sub(signal["after"].as_u64().unwrap());
    let upper = end.saturating_sub(signal["before"].as_u64().unwrap());
    assert!(
        upper < 5_000_000_000,
        "shutdown notice exceeded five seconds"
    );
    (String::from_utf8(bytes).unwrap(), [lower, upper])
}

struct Load {
    completed_before_controls: i64,
    completed: i64,
    claimed: i64,
    control_seconds: Vec<f64>,
}

fn measure_control_load(f: &Harness) -> Load {
    let db = rusqlite::Connection::open(f.root.join("runtime/runtime.sqlite")).unwrap();
    let counts = || -> (i64, i64, i64) {
        db.query_row(
            "SELECT count(*),sum(task_outcome='completed'),sum(claimed) FROM agents",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
    };
    support::wait::wait_until(
        Duration::from_secs(60),
        Duration::from_millis(10),
        "32 completed agents",
        || (counts().1 >= 32).then_some(()),
    );
    let completed_before_controls = counts().1;
    let mut control_seconds = Vec::new();
    for _ in 0..20 {
        let began = Instant::now();
        f.ok(json!({"op":"ping"}));
        control_seconds.push(began.elapsed().as_secs_f64());
        std::thread::sleep(Duration::from_millis(25));
    }
    let (total, completed, claimed) = counts();
    assert_eq!(total, 512);
    assert!(
        completed > completed_before_controls,
        "no completion during control window"
    );
    assert!(
        completed >= 32 && completed < total,
        "probe must signal under load"
    );
    assert!(claimed > 0);
    drop(db);
    Load {
        completed_before_controls,
        completed,
        claimed,
        control_seconds,
    }
}

fn export_probe(f: &Harness, result: &Value, raw: &str) {
    if let Some(output) = std::env::var_os("HIVE_RESPONSIVENESS_OUTPUT") {
        let output = PathBuf::from(output);
        fs::create_dir(&output).unwrap();
        fs::write(
            output.join("result.json"),
            serde_json::to_vec_pretty(result).unwrap(),
        )
        .unwrap();
        fs::write(output.join("hive-daemon.jsonl"), raw).unwrap();
        fs::copy(&f.config, output.join("config.yaml")).unwrap();
        fs::copy(
            f.root.join("runtime/runtime.sqlite"),
            output.join("runtime.sqlite"),
        )
        .unwrap();
    }
}
