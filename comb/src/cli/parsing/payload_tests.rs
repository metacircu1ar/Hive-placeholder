use super::{parse, tests::rejected};
use comb::store::input;
use std::ffi::OsString;

#[test]
fn scalar_payload_limits_and_whitespace_are_checked_before_files() {
    for (base, field, bound) in [
        (vec!["capture"], "TEXT", input::EVIDENCE_TEXT_BYTES),
        (
            vec!["append", "page", "--source", "source"],
            "TEXT",
            input::EVIDENCE_TEXT_BYTES,
        ),
        (
            vec!["append", "page", "text", "--source"],
            "--source",
            input::SOURCE_BYTES,
        ),
        (
            vec![
                "decide",
                "review/page",
                "accept",
                "--expected-hash",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "--reason",
            ],
            "--reason",
            input::DECISION_REASON_BYTES,
        ),
    ] {
        for value in [String::new(), " \n\t".into(), "x".repeat(bound + 1)] {
            let mut args = base.clone();
            args.push(&value);
            let output = rejected(&args);
            assert_eq!(output.exit_code, 2);
            let json: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
            assert_eq!(json["error"]["field"], field);
        }
        let value = "x".repeat(bound);
        let args: Vec<_> = ["comb"]
            .into_iter()
            .chain(base)
            .chain([value.as_str()])
            .map(OsString::from)
            .collect();
        assert!(parse(&args).is_ok());
    }
    let output = rejected(&[
        "append",
        "page",
        " ",
        "--source",
        "valid",
        "--summary-file",
        "PRIVATE-missing",
        "--expected-hash",
        &"a".repeat(64),
    ]);
    assert!(output.stdout.contains("TEXT"));
    assert!(!output.stdout.contains("PRIVATE"));
}

#[test]
fn decision_and_consolidation_use_native_prefix_membership() {
    let hash = "a".repeat(64);
    for (note, target, field) in [
        ("inbox", "page", "NOTE"),
        ("page", "page", "NOTE"),
        ("inbox/n", "inbox/target", "TARGET"),
        ("inbox/n", "review/target", "TARGET"),
    ] {
        let output = rejected(&[
            "--run",
            "run",
            "consolidate",
            note,
            target,
            "--note-hash",
            &hash,
            "--target-hash",
            &hash,
            "--summary-file",
            "PRIVATE-missing",
        ]);
        let json: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
        assert_eq!(json["error"]["field"], field);
    }
    for slug in ["review", "concepts/page"] {
        assert_eq!(
            rejected(&[
                "decide",
                slug,
                "accept",
                "--reason",
                "valid",
                "--expected-hash",
                &hash
            ])
            .exit_code,
            2
        );
    }
    for target in ["page", "inbox", "review"] {
        let args = [
            "comb",
            "--run",
            "run",
            "consolidate",
            "inbox/n",
            target,
            "--note-hash",
            &hash,
            "--target-hash",
            &hash,
            "--summary-file",
            "PRIVATE-missing",
        ];
        assert!(parse(&args.iter().map(OsString::from).collect::<Vec<_>>()).is_ok());
    }
}
