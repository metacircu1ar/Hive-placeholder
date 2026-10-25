use super::*;

#[test]
fn spawn_context_errors_reach_cli_and_control_without_admitting_agents() {
    let mut f = Harness::new();
    let context = f.tmp.path().join("context.md");
    fs::write(&context, "x".repeat(65534)).unwrap();
    f.edit_config(|config| {
        config["context"] = json!([context, context, context, context]);
    });
    let selected = f.tmp.path().join("private-canary-instructions.md");
    fs::write(&selected, "content-must-not-leak").unwrap();
    f.start();
    let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
        .arg("--state-dir")
        .arg(&f.root)
        .args(["spawn", "implementor", "--body", "work", "--cwd"])
        .arg(f.tmp.path())
        .arg("--instructions-file")
        .arg(&selected)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["error"]["code"], "out_of_range");
    assert_eq!(result["error"]["field"], "--instructions-file");
    assert!(!result.to_string().contains("private-canary"));
    assert!(!result.to_string().contains("content-must-not-leak"));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("Path: ") && stderr.contains("private-canary-instructions.md"));
    assert!(!stderr.contains("content-must-not-leak"));
    for cwd in [
        None,
        Some(json!("private-canary-relative")),
        Some(json!(f.tmp.path().join("private-canary-missing"))),
    ] {
        let mut raw = json!({"op":"spawn","role":"implementor","body":"work"});
        let code = if let Some(cwd) = cwd {
            raw["cwd"] = cwd;
            "invalid_path"
        } else {
            "missing_argument"
        };
        let response = f.expect_err(raw, code);
        assert_eq!(response["error"]["field"], "/cwd");
        assert!(!response.to_string().contains("private-canary"));
    }
    assert!(
        f.ok(json!({"op":"agents"}))["agents"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    f.shutdown();
}
