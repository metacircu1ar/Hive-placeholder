use super::*;
use crate::runtime::api;

#[test]
fn opaque_selection_fields_reject_empty_or_oversized_values_before_dispatch() {
    let cases: &[(&[&str], &str)] = &[
        (&["status", "@"], "TARGET"),
        (&["messages", "@"], "TARGET"),
        (&["read", "@"], "TARGET"),
        (&["send", "@", "--body-file", "absent"], "TARGET"),
        (&["complete", "@", "--result-file", "absent"], "TARGET"),
        (&["stop", "@"], "TARGET"),
        (&["resume", "@"], "TARGET"),
        (&["interrupt", "@"], "TARGET"),
        (&["terminate", "@"], "TARGET"),
        (&["purge", "@", "--dry-run"], "TARGET"),
        (&["set-auth", "@", "main"], "TARGET"),
        (&["set-options", "@", "--option", "model=mock"], "TARGET"),
        (&["spawn", "--from", "@", "--body-file", "absent"], "--from"),
        (&["agents", "--parent", "@"], "--parent"),
        (&["approvals", "--agent", "@"], "--agent"),
        (&["wait", "@"], "TARGET"),
        (&["wait", "--message", "@"], "--message"),
    ];
    for &(tail, field) in cases {
        for value in [String::new(), "é".repeat(32769)] {
            let args: Vec<_> = std::iter::once("hivectl")
                .chain(
                    tail.iter()
                        .map(|&part| if part == "@" { &value } else { part }),
                )
                .collect();
            let error = error(&args);
            assert_eq!(error["field"], field);
            assert_eq!(error["code"], "out_of_range");
            assert!(error.get("received").is_none());
        }
    }
    for value in [
        "x".repeat(65536),
        "é".repeat(32768),
        " private\nagent ".into(),
    ] {
        assert!(parse(&argv(&["hivectl", "status", &value]), false).is_ok());
    }
}

#[test]
fn configured_selections_share_config_name_validation_without_echoing() {
    let cases: &[(&[&str], &str)] = &[
        (&["spawn", "@", "--cwd", "/", "--body", "work"], "ROLE"),
        (
            &["spawn", "--auth", "@", "--cwd", "/", "--body", "work"],
            "--auth",
        ),
        (&["set-auth", "agent", "@"], "AUTH"),
        (&["agents", "--auth", "@"], "--auth"),
        (&["models", "--auth", "@"], "--auth"),
        (&["auth", "status", "@"], "NAME"),
        (&["auth", "login", "@"], "NAME"),
        (&["auth", "recover", "@"], "NAME"),
        (&["auth", "generations", "@"], "NAME"),
        (&["auth", "repair", "@", "--reauthenticate"], "NAME"),
        (
            &[
                "auth",
                "prune",
                "@",
                "--generation",
                "00000000-0000-4000-8000-000000000001",
            ],
            "NAME",
        ),
    ];
    for &(tail, field) in cases {
        for value in [
            String::new(),
            "private.canary".into(),
            "a".repeat(65),
            "a".repeat(64),
        ] {
            let args: Vec<_> = std::iter::once("hivectl")
                .chain(
                    tail.iter()
                        .map(|&part| if part == "@" { &value } else { part }),
                )
                .collect();
            if crate::config::valid_name(&value) {
                assert!(parse(&argv(&args), false).is_ok());
            } else {
                let error = error(&args);
                assert_eq!(error["field"], field);
                assert_eq!(error["code"], "invalid_value");
                assert!(!error.to_string().contains("private.canary"));
                assert!(error.get("received").is_none());
            }
        }
    }
}

#[test]
fn wait_join_checks_the_composed_read_target_bound() {
    let id = "x".repeat(65536 - "join:".len());
    assert!(parse(&argv(&["hivectl", "wait", "--join", &id]), false).is_ok());
    let wire: api::ReadArgs =
        serde_json::from_value(serde_json::json!({"target":format!("join:{id}")})).unwrap();
    assert!(
        wire.target
            .required(api::InputContext::Read, "/target")
            .is_ok()
    );
    for id in [String::new(), format!("{id}x")] {
        let error = error(&["hivectl", "wait", "--join", &id]);
        assert_eq!(error["field"], "--join");
        assert_eq!(error["code"], "out_of_range");
    }
}

#[test]
fn stalled_cursor_errors_are_private_and_precise_before_dispatch() {
    for (raw, code) in [
        (serde_json::json!([1, "private-canary"]), "invalid_value"),
        (
            serde_json::json!({"idle_since":1,"root_id":"","private-canary":true}),
            "invalid_value",
        ),
        (
            serde_json::json!({"idle_since":1,"root_id":""}),
            "out_of_range",
        ),
        (
            serde_json::json!({"idle_since":1,"root_id":"é".repeat(65)}),
            "out_of_range",
        ),
        (
            serde_json::json!({"idle_since":u64::MAX,"root_id":"private-canary"}),
            "out_of_range",
        ),
    ] {
        let error = error(&[
            "hivectl",
            "agents",
            "--stalled",
            "1s",
            "--stalled-after",
            &raw.to_string(),
        ]);
        assert_eq!(error["field"], "--stalled-after");
        assert_eq!(error["code"], code);
        assert!(!error.to_string().contains("private-canary"));
        assert!(error.get("received").is_none());
    }
}

#[cfg(feature = "comb")]
#[test]
fn collector_read_only_retry_id_is_rejected_and_mutation_names_are_validated() {
    for args in [
        vec![
            "hivectl",
            "collectors",
            "--request-id",
            "private-canary",
            "status",
        ],
        vec![
            "hivectl",
            "collectors",
            "status",
            "--request-id",
            "private-canary",
        ],
    ] {
        let error = error(&args);
        assert_eq!(error["code"], "conflicting_arguments");
        assert_eq!(error["field"], "--request-id");
        assert!(!error.to_string().contains("private-canary"));
    }
    for args in [
        vec![
            "hivectl",
            "collectors",
            "--request-id",
            "retry",
            "discard",
            "project",
        ],
        vec![
            "hivectl",
            "collectors",
            "discard",
            "project",
            "--request-id",
            "retry",
        ],
    ] {
        assert!(parse(&argv(&args), false).is_ok());
    }
    assert_eq!(
        error(&["hivectl", "collectors", "discard", "private.canary"])["field"],
        "DESTINATION"
    );
    assert_eq!(
        error(&["hivectl", "collectors", "release-uncertain", "project", ""])["field"],
        "ITEM"
    );
}

#[cfg(feature = "comb")]
#[test]
fn evidence_uses_canonical_verdict_without_a_legacy_alias() {
    assert!(
        parse(
            &argv(&[
                "hivectl",
                "evidence",
                "review",
                "--summary",
                "Public review.",
                "--verdict",
                "no_findings"
            ]),
            false
        )
        .is_ok()
    );
    let error = error(&[
        "hivectl",
        "evidence",
        "review",
        "--summary",
        "Public review.",
        "--verdict",
        "no-findings",
    ]);
    assert_eq!(error["field"], "--verdict");
    assert!(error["expected"].as_str().unwrap().contains("no_findings"));
}
