use super::*;

fn cli(f: &Harness, args: &[&str]) -> (std::process::Output, Value) {
    let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
        .arg("--state-dir")
        .arg(&f.root)
        .args(args)
        .output()
        .unwrap();
    let value = serde_json::from_slice(&output.stdout).unwrap();
    (output, value)
}

#[test]
fn collector_cli_errors_name_arguments_and_canonical_verdict_succeeds() {
    let mut f = Harness::new();
    let credential = f.tmp.path().join("unused.json");
    f.edit_config(|config| {
        config["comb_collectors"] =
            json!({"shared":{"credential_file":credential,"sources":["review"],"enabled":false}});
    });
    f.start();
    for (args, field) in [
        (
            vec!["collectors", "discard", "private-canary"],
            "DESTINATION",
        ),
        (
            vec![
                "collectors",
                "release-uncertain",
                "shared",
                "private-canary",
            ],
            "ITEM",
        ),
        (
            vec![
                "collectors",
                "rebind",
                "private-canary",
                "00000000-0000-4000-8000-000000000001",
            ],
            "DESTINATION",
        ),
    ] {
        let (output, value) = cli(&f, &args);
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert_eq!(value["error"]["code"], "invalid_value");
        assert_eq!(value["error"]["field"], field);
        assert!(!value.to_string().contains("private-canary"));
        assert!(String::from_utf8(output.stderr).unwrap().contains(field));
    }
    let error = f.expect_err(json!({"op":"collector_admin", "action":"rebind", "destination":"shared", "comb_id":"private-canary"}), "invalid_uuid");
    assert_eq!(error["error"]["field"], "/comb_id");
    assert!(!error.to_string().contains("private-canary"));
    let (output, _) = cli(
        &f,
        &[
            "evidence",
            "review",
            "--summary",
            "Public review.",
            "--verdict",
            "no_findings",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    let (output, value) = cli(
        &f,
        &[
            "evidence",
            "review",
            "--summary",
            "Public review.",
            "--verdict",
            "no-findings",
        ],
    );
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(value["error"]["field"], "--verdict");
    assert_eq!(
        f.ok(json!({"op":"collectors"}))["collectors"][0]["pending"],
        0
    );
    f.shutdown();
}
