use super::*;

#[test]
fn compact_ranges_are_explicit_and_preview_accepts_the_bounds() {
    for (field, value) in [
        ("--before", "9223372036854775808"),
        ("--keep-per-agent", "0"),
        ("--keep-per-agent", "18446744073709551615"),
    ] {
        let mut args = vec!["hivectl", "compact"];
        if field != "--before" {
            args.extend(["--before", "0"]);
        }
        args.extend([field, value]);
        let invalid = error(&args);
        assert_eq!(invalid["code"], "out_of_range");
        assert_eq!(invalid["field"], field);
    }
    for (before, retained) in [("0", "1"), ("9223372036854775807", "9223372036854775807")] {
        assert!(
            parse(
                &argv(&[
                    "hivectl",
                    "compact",
                    "--before",
                    before,
                    "--keep-per-agent",
                    retained,
                    "--dry-run"
                ]),
                false
            )
            .is_ok()
        );
    }
}

#[test]
fn all_request_id_arguments_use_the_same_private_bounded_parser() {
    fn walk(command: &clap::Command, count: &mut usize) {
        for arg in command
            .get_arguments()
            .filter(|arg| arg.get_id() == "request_id")
        {
            *count += 1;
            let parser = clap::Command::new("test").arg(
                clap::Arg::new("value")
                    .long("value")
                    .value_parser(arg.get_value_parser().clone()),
            );
            for value in [
                String::new(),
                "x".repeat(129),
                "é".repeat(65),
                "private\nline".into(),
                "private\u{85}line".into(),
                "é".repeat(64),
                "x".repeat(128),
                "retry 1".into(),
            ] {
                assert_eq!(
                    parser
                        .clone()
                        .try_get_matches_from(["test", "--value", &value])
                        .is_ok(),
                    hive_input::validate::request_id(&value).is_ok(),
                    "{}",
                    command.get_name()
                );
            }
        }
        for child in command
            .get_subcommands()
            .filter(|child| child.get_name() != "help")
        {
            walk(child, count);
        }
    }
    let mut command = schema(false);
    command.build();
    let mut count = 0;
    walk(&command, &mut count);
    assert!(count >= 9);
    let value = error(&[
        "hivectl",
        "send",
        "agent",
        "--body-file",
        "absent",
        "--request-id",
        "PRIVATE\nID",
    ]);
    assert_eq!(value["code"], "invalid_request_id");
    assert_eq!(value["field"], "--request-id");
    assert!(value.get("received").is_none());
}

#[test]
fn model_and_effort_have_specific_early_errors_and_provider_aliases_parse() {
    for model in ["sonnet[1m]", "provider/model:revision@alias", "gpt-6-luna"] {
        assert!(
            parse(
                &argv(&[
                    "hivectl",
                    "set-options",
                    "agent",
                    "--option",
                    &format!("model={model}"),
                    "--option",
                    "effort=high"
                ]),
                false
            )
            .is_ok()
        );
    }
    for args in [
        vec![
            "hivectl", "spawn", "--cwd", "/absent", "--body", "hello", "--model", "bad name",
        ],
        vec!["hivectl", "setup", "--claude-model", "bad name"],
        vec![
            "hivectl",
            "verify-steering",
            "--auth",
            "main",
            "--model",
            "bad name",
            "--effort",
            "low",
        ],
    ] {
        let value = error(&args);
        assert_eq!(value["code"], "invalid_model");
        assert_eq!(value["received"], "bad name");
    }
    for args in [
        vec!["hivectl", "setup", "--codex-effort", "fast"],
        vec![
            "hivectl", "spawn", "--cwd", "/absent", "--body", "hello", "--effort", "fast",
        ],
    ] {
        assert_eq!(error(&args)["code"], "invalid_effort");
    }
}

#[test]
fn uuid_fields_reject_unstructured_values_without_echoing_them() {
    let mut cases = vec![
        vec!["hivectl", "output", "PRIVATE"],
        vec!["hivectl", "approvals", "show", "PRIVATE"],
        vec!["hivectl", "respond", "PRIVATE", "--accept"],
        vec![
            "hivectl",
            "auth",
            "prune",
            "main",
            "--generation",
            "PRIVATE",
        ],
    ];
    if cfg!(feature = "comb") {
        cases.push(vec![
            "hivectl",
            "collectors",
            "rebind",
            "project",
            "PRIVATE",
        ]);
    }
    for args in cases {
        let value = error(&args);
        assert_eq!(value["code"], "invalid_uuid");
        assert!(value.get("received").is_none());
        assert!(!value.to_string().contains("PRIVATE"));
    }
    let raw = "0000000000004000800000000000000A";
    assert_eq!(hive_input::clap::parsers::uuid(raw).unwrap(), raw);
}

#[test]
fn message_constraints_reject_before_dispatch_and_preserve_redirect_calls() {
    for delay in ["0s", "604801s", "8d", "18446744073709551615w"] {
        assert_eq!(
            error(&[
                "hivectl",
                "send",
                "agent",
                "--body-file",
                "absent",
                "--delay",
                delay
            ])["code"],
            "invalid_duration"
        );
    }
    for mode in ["steer", "redirect"] {
        let value = error(&[
            "hivectl",
            "send",
            "agent",
            "--body-file",
            "absent",
            "--delay",
            "1s",
            "--mode",
            mode,
        ]);
        assert_eq!(value["code"], "conflicting_arguments");
        assert_eq!(value["field"], "--mode");
    }
    assert_eq!(
        error(&[
            "hivectl",
            "send",
            "agent",
            "--body-file",
            "absent",
            "--wait",
            "--mode",
            "steer"
        ])["code"],
        "conflicting_arguments"
    );
    for args in [
        vec![
            "hivectl",
            "send",
            "agent",
            "--body-file",
            "absent",
            "--wait",
            "--mode",
            "redirect",
        ],
        vec![
            "hivectl",
            "send",
            "agent",
            "--body-file",
            "absent",
            "--delay",
            "1s",
        ],
        vec![
            "hivectl",
            "send",
            "agent",
            "--body-file",
            "absent",
            "--delay",
            "7d",
        ],
        vec!["hivectl", "wait", "--message", "m", "--timeout", "600"],
    ] {
        assert!(parse(&argv(&args), false).is_ok());
    }
    let value = error(&["hivectl", "wait", "--message", "m", "--timeout", "601"]);
    assert_eq!(value["code"], "out_of_range");
    assert_eq!(value["field"], "--timeout");
}
