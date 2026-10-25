use super::{parse, tests::rejected};
use serde_json::Value;
use std::ffi::OsString;

fn accepted(args: &[&str]) {
    assert!(
        parse(
            &std::iter::once("comb")
                .chain(args.iter().copied())
                .map(OsString::from)
                .collect::<Vec<_>>()
        )
        .is_ok(),
        "{args:?}"
    );
}
fn invalid(args: &[&str], field: &str, expected: &str) {
    let output = rejected(args);
    assert_eq!(output.exit_code, 2, "{args:?}: {}", output.stderr);
    let json: Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(json["error"]["field"], field, "{args:?}");
    assert!(
        json["error"]["expected"]
            .as_str()
            .unwrap()
            .contains(expected),
        "{json}"
    );
    assert!(!json["error"]["examples"].as_array().unwrap().is_empty());
    assert!(json["error"].get("received").is_none());
    assert!(!output.stdout.contains("PRIVATE"));
    assert!(!output.stderr.contains("PRIVATE"));
}

#[test]
fn every_numeric_bound_rejects_zero_or_excess_and_keeps_endpoints() {
    for (base, flag, min, max) in [
        (vec!["search", "text"], "--limit", 1, 100),
        (vec!["list"], "--limit", 1, 1000),
        (vec!["links", "concepts/page"], "--limit", 1, 1000),
        (vec!["backlinks", "concepts/page"], "--limit", 1, 1000),
        (vec!["graph", "concepts/page"], "--depth", 0, 8),
        (vec!["serve"], "--vector-cache-mib", 1, 8192),
    ] {
        for bound in [min, max] {
            let text = bound.to_string();
            let mut args = base.clone();
            args.extend([flag, &text]);
            accepted(&args);
        }
        for text in [
            (max + 1).to_string(),
            "184467440737095516160".into(),
            "+1".into(),
            " 1".into(),
        ] {
            let mut args = base.clone();
            args.extend([flag, &text]);
            invalid(&args, flag, "decimal integer");
        }
        if min > 0 {
            let mut args = base.clone();
            args.extend([flag, "0"]);
            invalid(&args, flag, "decimal integer");
        }
    }
}

#[test]
fn primitive_rules_are_specific_and_never_echo_knowledge() {
    for (args, field, expected) in [
        (vec!["get", "PRIVATE"], "SLUG", "slash-separated"),
        (
            vec!["put", "page", "--expected-hash", "PRIVATE"],
            "--expected-hash",
            "64 lowercase",
        ),
        (
            vec!["capture", "text", "--request-id", "PRIVATE\n"],
            "--request-id",
            "1..128",
        ),
        (
            vec!["--run", "PRIVATE\n", "capture", "text"],
            "--run",
            "1..512",
        ),
        (vec!["credential", "revoke", "PRIVATE"], "ID", "UUID"),
        (
            vec!["credential", "create", "PRIVATE\n", "--out", "PRIVATE.json"],
            "NAME",
            "1..512",
        ),
        (vec!["search", " \n"], "QUERY", "nonblank"),
        (vec!["list", "--prefix", ""], "--prefix", "1..240"),
        (vec!["list", "--type", ""], "--type", "1..256"),
        (vec!["list", "--tag", ""], "--tag", "1..256"),
        (
            vec!["search", "text", "--after", "PRIVATE"],
            "--after",
            "RFC3339",
        ),
        (
            vec!["list", "--after", "PRIVATE"],
            "--after",
            "slash-separated",
        ),
        (
            vec!["links", "page", "--after", "PRIVATE"],
            "--after",
            "slash-separated",
        ),
        (
            vec!["import", "PRIVATE", "--prefix", "PRIVATE/"],
            "--prefix",
            "slash-separated",
        ),
    ] {
        invalid(&args, field, expected);
    }
    accepted(&[
        "list",
        "--prefix",
        "components/",
        "--after",
        "components/page",
    ]);
    accepted(&[
        "search",
        "query",
        "--after",
        "2026-01-01T01:00:00+01:00",
        "--before",
        "2026-01-01T00:00:00Z",
    ]);
    accepted(&["serve", "--replace-embeddings", "--retry-failed"]);
}

#[test]
fn relationships_and_raw_role_lists_are_checked_before_runtime() {
    invalid(
        &[
            "search",
            "text",
            "--mode",
            "text",
            "--embedding-config",
            "PRIVATE.yaml",
        ],
        "--embedding-config",
        "hybrid",
    );
    invalid(
        &[
            "search",
            "text",
            "--after",
            "2026-01-01T00:00:00Z",
            "--before",
            "2026-01-01T00:59:59+01:00",
        ],
        "--after",
        "at or before",
    );
    invalid(&["init", "PRIVATE", "--root", "PRIVATE"], "DIR", "--root");
    for base in [
        vec![
            "credential",
            "create",
            "label",
            "--out",
            "PRIVATE.json",
            "--bridge",
        ],
        vec!["setup", "--for-hive", "--no-services"],
    ] {
        let mut duplicate = base.clone();
        duplicate.extend(["--reviewer-role", "PRIVATE", "--reviewer-role", "PRIVATE"]);
        invalid(&duplicate, "--reviewer-role", "unique");
        let roles: Vec<_> = (0..65).map(|i| format!("role{i}")).collect();
        let mut args = base;
        for role in &roles[..64] {
            args.extend(["--reviewer-role", role]);
        }
        accepted(&args);
        args.extend(["--reviewer-role", &roles[64]]);
        invalid(&args, "--reviewer-role", "at most 64");
    }
}

#[test]
fn global_relationships_work_before_or_after_the_subcommand() {
    for args in [
        vec!["--root", "PRIVATE", "init", "PRIVATE"],
        vec!["init", "PRIVATE", "--root", "PRIVATE"],
    ] {
        invalid(&args, "DIR", "--root");
    }
    let hash = "a".repeat(64);
    let base = vec![
        "consolidate",
        "inbox/note",
        "page",
        "--note-hash",
        &hash,
        "--target-hash",
        &hash,
        "--summary-file",
        "PRIVATE.md",
    ];
    invalid(&base, "--run", "explicit run");
    let mut before = vec!["--run", "run"];
    before.extend_from_slice(&base);
    accepted(&before);
    let mut after = base;
    after.extend(["--run", "run"]);
    accepted(&after);
}
