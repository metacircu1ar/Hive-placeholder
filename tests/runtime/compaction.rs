use super::*;

fn cli(f: &Harness, args: &[&str]) -> (std::process::Output, Value) {
    let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
        .arg("--state-dir")
        .arg(&f.root)
        .args(args)
        .output()
        .unwrap();
    let result = serde_json::from_slice(&output.stdout).unwrap();
    (output, result)
}

#[test]
fn offline_cli_preview_preserves_mock_history_then_counts_actual_pruning() {
    let mut f = Harness::new();
    f.start();
    let id = f.spawn("idle");
    f.settled(&id);
    f.ok(json!({"op":"complete","target":id,"body":"done"}));
    // Session release may precede the shared process's final shutdown event.
    f.wait(&id, |a| a["state"] == "stopped" && !f.reports().is_empty());
    let (busy, _) = cli(
        &f,
        &["compact", "--before", "9223372036854775807", "--dry-run"],
    );
    assert!(!busy.status.success());
    let before = f.read(&id);
    f.shutdown();
    let args = [
        "compact",
        "--before",
        "9223372036854775807",
        "--keep-per-agent",
        "1",
        "--dry-run",
    ];
    let (output, preview) = cli(&f, &args);
    assert!(output.status.success(), "{preview}");
    assert_eq!(preview["dry_run"], true);
    assert!(preview["events_to_remove"].as_u64().unwrap() > 0);
    assert!(!f.root.join("runtime/output-manifest.jsonl").exists());
    f.start();
    assert_eq!(f.read(&id), before);
    f.shutdown();
    let (output, result) = cli(&f, &args[..args.len() - 1]);
    assert!(output.status.success(), "{result}");
    assert_eq!(result["events_removed"], preview["events_to_remove"]);
    assert_eq!(
        result["call_results_removed"],
        preview["call_results_to_remove"]
    );
    assert_eq!(
        result["orphan_artifacts_removed"],
        preview["orphan_artifacts_to_remove"]
    );
    assert!(f.root.join("runtime/output-manifest.jsonl").exists());
}
