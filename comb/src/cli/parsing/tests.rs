use super::*;
use serde_json::Value;

pub(super) fn rejected(args: &[&str]) -> Rendered {
    let args: Vec<_> = std::iter::once("comb")
        .chain(args.iter().copied())
        .map(OsString::from)
        .collect();
    match parse(&args) {
        Ok(_) => panic!("expected parser rejection"),
        Err(output) => output,
    }
}

#[test]
fn explicit_service_search_paths_reject_bad_entries_before_setup_or_service_work() {
    for path in ["/bin:relative", "/bin:", "/missing-toolchain", ""] {
        for prefix in [vec!["setup"], vec!["service", "install"]] {
            let mut args = prefix;
            args.extend(["--service-path", path]);
            let result = rejected(&args);
            assert_eq!(result.exit_code, 2);
            let value: Value = serde_json::from_str(&result.stdout).unwrap();
            assert_eq!(value["error"]["field"], "--service-path");
        }
    }
}

#[test]
fn every_visible_command_has_help_and_documented_parseable_examples() {
    fn check(
        command: &clap::Command,
        path: &str,
        guide: &str,
        paths: &mut std::collections::BTreeSet<String>,
    ) {
        assert!(paths.insert(path.to_owned()), "duplicate command: {path}");
        assert!(command.get_about().is_some(), "{path}");
        let examples = catalog::examples(path).unwrap();
        assert!(command.get_after_help().is_some(), "{path}");
        for example in examples.as_slice() {
            assert!(guide.contains(example), "missing guide example: {example}");
            let result = parse(
                &example
                    .split_whitespace()
                    .map(OsString::from)
                    .collect::<Vec<_>>(),
            );
            assert!(
                result.is_ok() || result.err().is_some_and(|e| e.exit_code == 0),
                "{example}"
            );
        }
        for arg in command.get_arguments().filter(|arg| !arg.is_hide_set()) {
            assert!(arg.get_help().is_some(), "{path}: {}", arg.get_id());
        }
        for child in command
            .get_subcommands()
            .filter(|c| !c.is_hide_set() && c.get_name() != "help")
        {
            let path = format!("{path} {}", child.get_name());
            check(child, path.trim(), guide, paths);
        }
    }
    let mut command = schema();
    command.build();
    let mut paths = std::collections::BTreeSet::new();
    check(&command, "", include_str!("../../../README.md"), &mut paths);
    let registry: std::collections::BTreeSet<_> = catalog::COMMANDS
        .iter()
        .map(|(path, _, _)| (*path).to_owned())
        .collect();
    assert_eq!(
        catalog::COMMANDS.len(),
        registry.len(),
        "duplicate example key"
    );
    assert_eq!(paths, registry, "stale or missing example key");
}

#[test]
fn selected_command_errors_are_structured_without_private_values() {
    for (args, example) in [
        (
            vec![
                "credential",
                "create",
                "private-client",
                "--bridge",
                "--reviewer",
                "--out",
                "private.json",
            ],
            "comb credential create ",
        ),
        (
            vec!["--root", "model", "capture", "--private-unknown"],
            "comb capture ",
        ),
        (
            vec![
                "model",
                "use-http",
                "--url",
                "https://private.invalid/SECRET",
                "--model",
                "private-model",
                "--dimensions",
                "bad",
            ],
            "comb model use-http ",
        ),
        (
            vec![
                "--run",
                "credential",
                "search",
                "private-query",
                "--mode",
                "private-mode",
            ],
            "comb search ",
        ),
        (
            vec!["setup-maintenance", "--private-unknown"],
            "comb setup ",
        ),
        (vec!["mcp"], "comb mcp "),
    ] {
        let output = input_error(&args, example);
        for secret in [
            "private-client",
            "private.json",
            "private-unknown",
            "private.invalid",
            "SECRET",
            "private-model",
            "private-query",
            "private-mode",
        ] {
            assert!(!output.stdout.contains(secret), "stdout: {secret}");
            assert!(!output.stderr.contains(secret), "stderr: {secret}");
        }
    }
}

fn input_error(args: &[&str], example: &str) -> Rendered {
    let output = rejected(args);
    assert_eq!(output.exit_code, 2, "{args:?}");
    let value: Value = serde_json::from_str(&output.stdout).unwrap();
    let error = &value["error"];
    for key in ["code", "message", "field", "expected"] {
        assert!(!error[key].as_str().unwrap().is_empty(), "{key}: {value}");
    }
    assert!(error.get("received").is_none());
    let text = error["examples"][0].as_str().unwrap();
    assert!(text.starts_with(example), "{text}");
    assert!(output.stderr.contains(text));
    output
}

#[test]
fn public_numeric_received_is_escaped_and_local_only() {
    let output = rejected(&[
        "search",
        "query",
        "--limit",
        "999999999999999999999\n\x1b[2J",
    ]);
    assert_eq!(output.exit_code, 2);
    let value: Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(value["error"]["field"], "--limit");
    assert!(value["error"].get("received").is_none());
    assert!(output.stderr.contains("Received: "));
    assert!(!output.stderr.contains('\x1b'));
    assert!(!output.stdout.contains("999999"));
}

#[test]
fn missing_values_before_informational_flags_keep_input_error_and_command_context() {
    for information in ["--help", "--version"] {
        for (args, field, example) in [
            (vec!["--root", information], "--root", "comb --help"),
            (
                vec!["model", "use-http", "--url", information],
                "--url",
                "comb model use-http ",
            ),
            (
                vec!["credential", "create", "private-name", "--out", information],
                "--out",
                "comb credential create ",
            ),
        ] {
            let output = input_error(&args, example);
            let value: Value = serde_json::from_str(&output.stdout).unwrap();
            if information == "--version" && field != "--root" {
                // Child commands do not inherit --version. Preserve the real
                // parser's unknown-argument outcome instead of adding support.
                assert_eq!(value["error"]["code"], "unknown_argument");
                assert_eq!(value["error"]["field"], "arguments");
            } else {
                assert_eq!(value["error"]["field"], field);
            }
        }
    }
}

#[test]
fn help_version_and_missing_subcommands_have_distinct_outcomes() {
    for args in [
        &["--help"][..],
        &["model", "use-http", "--help"],
        &["--version"],
    ] {
        let output = rejected(args);
        assert_eq!(output.exit_code, 0);
        assert!(!output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }
    for args in [&[][..], &["model"], &["credential"]] {
        let output = rejected(args);
        assert_eq!(output.exit_code, 2);
        let value: Value = serde_json::from_str(&output.stdout).unwrap();
        assert_eq!(value["error"]["code"], "missing_argument");
    }
}

#[test]
fn checked_parser_output_attempts_both_channels_and_observes_flush() {
    struct Writer {
        bytes: Vec<u8>,
        fail_after: Option<usize>,
        fail_flush: bool,
    }
    impl Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let remaining = self
                .fail_after
                .map_or(3, |cap| cap.saturating_sub(self.bytes.len()));
            if remaining == 0 {
                return Err(std::io::Error::other("injected write"));
            }
            let count = bytes.len().min(3).min(remaining);
            self.bytes.extend_from_slice(&bytes[..count]);
            Ok(count)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            if self.fail_flush {
                Err(std::io::Error::other("injected flush"))
            } else {
                Ok(())
            }
        }
    }
    let output = rejected(&["--unknown"]);
    for (fail_after, flush) in [
        (None, false),
        (Some(0), false),
        (Some(3), false),
        (None, true),
    ] {
        let mut stdout = Writer {
            bytes: Vec::new(),
            fail_after,
            fail_flush: flush,
        };
        let mut stderr = Vec::new();
        let code = emit_to(&output, &mut stdout, &mut stderr);
        assert_eq!(code, if fail_after.is_some() || flush { 1 } else { 2 });
        assert_eq!(stderr, output.stderr.as_bytes());
        assert_eq!(
            stdout.bytes,
            &output.stdout.as_bytes()[..fail_after.unwrap_or(output.stdout.len())]
        );
    }
    let mut stderr = Writer {
        bytes: Vec::new(),
        fail_after: Some(0),
        fail_flush: false,
    };
    let mut stdout = Vec::new();
    assert_eq!(emit_to(&output, &mut stdout, &mut stderr), 1);
    assert_eq!(stdout, output.stdout.as_bytes());
    let help = rejected(&["--help"]);
    assert_eq!(emit_to(&help, &mut Vec::new(), &mut stderr), 0);
}
