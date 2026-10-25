use super::*;

#[test]
fn page_limits_reject_without_clamping_and_keep_boundary_values() {
    for prefix in [
        vec!["hivectl", "agents"],
        vec!["hivectl", "agents", "--stalled", "1s"],
        vec!["hivectl", "messages", "agent"],
        vec!["hivectl", "read", "agent"],
        vec!["hivectl", "read", "agent", "--follow"],
    ] {
        for (limit, valid) in [("0", false), ("1", true), ("200", true), ("201", false)] {
            let mut args = prefix.clone();
            args.extend(["--limit", limit]);
            if valid {
                assert!(parse(&argv(&args), false).is_ok(), "{args:?}");
            } else {
                let value = error(&args);
                assert_eq!(value["field"], "--limit");
                // Clap's native ranged parser uses invalid_value; conditional
                // target-specific read limits use the shared out_of_range code.
                assert!(matches!(
                    value["code"].as_str(),
                    Some("invalid_value" | "out_of_range")
                ));
            }
        }
    }
    for prefix in [
        vec!["hivectl", "output", "00000000-0000-4000-8000-000000000001"],
        vec![
            "hivectl",
            "read",
            "artifact:00000000-0000-4000-8000-000000000001",
        ],
    ] {
        for (limit, valid) in [
            ("0", false),
            ("3", false),
            ("4", true),
            ("65536", true),
            ("65537", false),
        ] {
            let mut args = prefix.clone();
            args.extend(["--limit", limit]);
            assert_eq!(parse(&argv(&args), false).is_ok(), valid, "{args:?}");
        }
    }
}

#[test]
fn read_omissions_reach_the_daemon_and_ignored_options_are_rejected() {
    for target in ["agent", "artifact:ID", "join:ID"] {
        let Ok(cli) = parse(&argv(&["hivectl", "read", target]), false) else {
            panic!("valid read");
        };
        let Command::Read(read) = cli.command else {
            panic!("read")
        };
        assert_eq!(
            serde_json::to_value(read.request()).unwrap(),
            serde_json::json!({"op":"read", "target":target})
        );
    }
    for (target, flag, value) in [("join:ID", "--after", "0"), ("join:ID", "--limit", "100")] {
        let value = error(&["hivectl", "read", target, flag, value]);
        assert_eq!(value["code"], "conflicting_arguments");
        assert_eq!(value["field"], flag);
    }
    for target in ["join:PRIVATE", "artifact:PRIVATE"] {
        let value = error(&["hivectl", "read", target, "--follow"]);
        assert_eq!(value["field"], "--follow");
        assert!(!value.to_string().contains("PRIVATE"));
    }
    for args in [
        vec!["hivectl", "wait", "agent", "--after", "0"],
        vec![
            "hivectl",
            "wait",
            "agent",
            "--until",
            "completed",
            "--after",
            "1",
        ],
    ] {
        let value = error(&args);
        assert_eq!(value["code"], "conflicting_arguments");
        assert_eq!(value["field"], "--after");
    }
    for args in [
        vec!["hivectl", "wait", "agent"],
        vec!["hivectl", "wait", "agent", "--until", "turn-finished"],
        vec![
            "hivectl",
            "wait",
            "agent",
            "--until",
            "turn-finished",
            "--after",
            "0",
        ],
    ] {
        assert!(parse(&argv(&args), false).is_ok());
    }
}
