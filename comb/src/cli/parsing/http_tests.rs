use super::{parse, tests::rejected};
use serde_json::Value;
use std::ffi::OsString;

fn args<'a>(flag: &'a str, value: &'a str) -> Vec<&'a str> {
    let mut args = vec!["model", "use-http"];
    for (name, default) in [
        ("--url", "http://127.0.0.1:9/v1/embeddings"),
        ("--model", "model"),
        ("--dimensions", "3"),
    ] {
        if name != flag {
            args.extend([name, default]);
        }
    }
    args.extend([flag, value]);
    args
}
fn accepted(args: &[&str]) {
    let result = parse(
        &std::iter::once("comb")
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect::<Vec<_>>(),
    );
    assert!(result.is_ok(), "{}", result.err().unwrap().stderr);
}
fn invalid(args: &[&str], field: &str, code: &str) {
    let output = rejected(args);
    assert_eq!(output.exit_code, 2, "{}", output.stderr);
    let value: Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(value["error"]["field"], field);
    assert_eq!(value["error"]["code"], code);
    assert!(value["error"]["expected"].is_string());
    assert!(
        value["error"]["examples"][0]
            .as_str()
            .unwrap()
            .starts_with("comb model use-http ")
    );
    for text in [&output.stdout, &output.stderr] {
        assert!(!text.contains("PRIVATE"));
        assert!(!text.contains('\u{1b}'));
    }
}

#[test]
fn http_ranges_and_tokenizer_pair_are_enforced_without_runtime() {
    for (flag, min, max) in [
        ("--dimensions", 1, 4096),
        ("--timeout-seconds", 1, 120),
        ("--max-tokens", 16, 8192),
    ] {
        for (n, valid) in [(min - 1, false), (min, true), (max, true), (max + 1, false)] {
            let value = n.to_string();
            let mut argv = args(flag, &value);
            if flag == "--max-tokens" {
                argv.extend(["--tokenizer", "PRIVATE-absent.json"]);
            }
            if valid {
                accepted(&argv);
            } else {
                invalid(&argv, flag, "out_of_range");
            }
        }
    }
    invalid(
        &args("--max-tokens", "16"),
        "--tokenizer",
        "missing_argument",
    );
    invalid(
        &args("--tokenizer", "PRIVATE.json"),
        "--max-tokens",
        "missing_argument",
    );
}

#[test]
fn http_identity_and_prefix_limits_keep_utf8_and_empty_prefixes() {
    for flag in ["--model", "--revision"] {
        invalid(&args(flag, ""), flag, "invalid_value");
        accepted(&args(flag, &"é".repeat(2048)));
        invalid(
            &args(flag, &format!("{}PRIVATE", "é".repeat(2048))),
            flag,
            "invalid_value",
        );
    }
    for flag in ["--query-prefix", "--document-prefix"] {
        accepted(&args(flag, ""));
        accepted(&args(flag, "Instruct: Retrieve knowledge\nQuery:"));
        accepted(&args(flag, &"é".repeat(512)));
        invalid(
            &args(flag, &format!("{}PRIVATE", "é".repeat(512))),
            flag,
            "invalid_value",
        );
    }
}

#[test]
fn http_url_diagnostics_preserve_policy_without_echoing_values() {
    for url in [
        "PRIVATE",
        "http://PRIVATE.invalid/v1/embeddings",
        "https://PRIVATE:secret@example.test/v1/embeddings",
        "https://example.test/v1/embeddings?PRIVATE=secret",
        "https://example.test/v1/embeddings#PRIVATE",
    ] {
        invalid(&args("--url", url), "--url", "invalid_value");
    }
    for url in [
        "https://example.test/v1/embeddings",
        "http://localhost:8081/v1/embeddings",
        "http://127.0.0.1:8081/v1/embeddings",
        "http://[::1]:8081/v1/embeddings",
    ] {
        accepted(&args("--url", url));
    }
    let oversized = format!("https://example.test/{}PRIVATE", "x".repeat(4096));
    invalid(&args("--url", &oversized), "--url", "invalid_value");
}
