use super::*;

fn cli_error(f: &Harness, args: &[&str], code: &str, field: &str) {
    let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
        .arg("--state-dir")
        .arg(&f.root)
        .args(args)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["error"]["code"], code, "{result}");
    assert_eq!(result["error"]["field"], field, "{result}");
    assert!(!result.to_string().contains("private-canary"));
    assert!(!result["error"]["examples"].as_array().unwrap().is_empty());
}

#[test]
fn cli_selection_errors_identify_arguments_without_changing_the_agent() {
    let mut f = Harness::builder(Provider::Claude).build();
    f.start();
    let id = f.spawn("hold");
    let original = f.wait(&id, |a| a["state"] == "running");
    let cwd = f.tmp.path().to_str().unwrap();
    for (args, code, field) in [
        (
            vec!["set-options", &id, "--option", "model=private-canary"],
            "invalid_value",
            "--option",
        ),
        (
            vec![
                "set-options",
                &id,
                "--option",
                "model=gpt-5.6-luna",
                "--option",
                "effort=low",
            ],
            "invalid_value",
            "--option",
        ),
        (
            vec!["set-auth", &id, "claude"],
            "conflicting_arguments",
            "AUTH",
        ),
        (
            vec!["set-auth", &id, "private-canary"],
            "invalid_value",
            "AUTH",
        ),
        (
            vec!["spawn", "private-canary", "--cwd", cwd, "--body", "work"],
            "invalid_value",
            "ROLE",
        ),
    ] {
        cli_error(&f, &args, code, field);
    }
    for (extra, field) in [
        (json!({"model":"private-canary model"}), "/options/model"),
        (
            json!({"model":"gpt-5.6-luna","effort":"private-canary"}),
            "/options/effort",
        ),
    ] {
        let request = json!({"op":"set_options", "target":id, "options":extra});
        let response = f.call(request);
        assert_eq!(response["error"]["field"], field, "{response}");
        assert!(!response.to_string().contains("private-canary"));
    }
    let current = f.status(&id);
    for field in [
        "options_revision",
        "model",
        "effort",
        "auth",
        "thread_id",
        "turn_id",
    ] {
        assert_eq!(current[field], original[field], "{field}");
    }
    assert_eq!(
        f.ok(json!({"op":"agents"}))["agents"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    f.shutdown();
}

#[test]
fn branch_catalog_misses_use_original_overrides_and_preserve_config_errors() {
    let mut f = Harness::new();
    f.edit_config(|config| {
        config["roles"]["uncatalogued"] = config["roles"]["implementor"].clone();
        config["roles"]["uncatalogued"]["model"] = json!("private-canary");
    });
    f.start();
    let source = f.spawn("reply:prefix");
    f.wait(&source, |a| {
        a["state"] == "idle" && !a["last_finished_turn"].is_null()
    });
    for (extra, field) in [
        (json!({"model":"private-canary"}), "/model"),
        (json!({"effort":"low"}), "/effort"),
    ] {
        let mut raw = extra;
        raw["op"] = json!("spawn");
        raw["from_agent"] = json!(source);
        raw["body"] = json!("work");
        let response = f.expect_err(raw, "invalid_value");
        assert_eq!(response["error"]["field"], field, "{response}");
        assert!(!response.to_string().contains("private-canary"));
    }
    let response = f.expect_err(
        json!({"op":"spawn","from_agent":source,"role":"uncatalogued","body":"work"}),
        "unknown_model",
    );
    assert!(response["error"]["field"].is_null(), "{response}");
    assert_eq!(
        f.ok(json!({"op":"agents"}))["agents"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    f.shutdown();
}
