#![allow(clippy::unwrap_used, clippy::expect_used)] // Pure parser fixtures fail on malformed metadata.

use super::*;
use std::{collections::BTreeSet, io};

mod coverage;
mod emission;
mod globals;
mod pages;
mod selections;
mod values;

fn argv(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

fn rejected(args: &[&str], daemon: bool) -> Exit {
    match parse(&argv(args), daemon) {
        Ok(_) => panic!("expected parser exit"),
        Err(exit) => exit,
    }
}

fn error(args: &[&str]) -> serde_json::Value {
    let exit = rejected(args, false);
    assert_eq!(exit.code, 2, "{}", exit.stderr);
    let value: serde_json::Value = serde_json::from_str(&exit.stdout).unwrap();
    for example in value["error"]["examples"].as_array().unwrap() {
        assert!(exit.stderr.contains(example.as_str().unwrap()));
    }
    value["error"].clone()
}

fn public_commands(command: &clap::Command, path: &str, paths: &mut BTreeSet<String>) {
    assert!(paths.insert(path.to_owned()), "duplicate command {path}");
    assert!(command.get_about().is_some(), "missing description: {path}");
    let examples = catalog::examples(path).expect(path);
    assert_eq!(
        command.get_after_help().unwrap().to_string(),
        examples.help()
    );
    let rules = rules::for_command(command, path);
    Context::new(command, &examples, &rules).expect(path);
    for child in command.get_subcommands() {
        // Clap generates help. setup-maintenance is private service plumbing,
        // not an independently supported public entrypoint.
        if child.get_name() == "help" || child.get_name() == "setup-maintenance" {
            continue;
        }
        assert!(!child.is_hide_set(), "unclassified hidden command");
        let next = if path.is_empty() {
            child.get_name().to_owned()
        } else {
            format!("{path} {}", child.get_name())
        };
        public_commands(child, &next, paths);
    }
}

#[test]
fn every_public_command_has_shared_examples_description_and_valid_field_rules() {
    let mut paths = BTreeSet::new();
    for daemon in [false, true] {
        let mut command = schema(daemon);
        command.build();
        public_commands(
            &command,
            if daemon { "hive-daemon" } else { "" },
            &mut paths,
        );
    }
    let registry: BTreeSet<_> = catalog::COMMANDS
        .iter()
        .map(|(path, _)| (*path).to_owned())
        .collect();
    assert_eq!(
        registry.len(),
        catalog::COMMANDS.len(),
        "duplicate example key"
    );
    assert_eq!(paths, registry);
}

#[test]
fn every_visible_argument_has_help() {
    fn visit(command: &clap::Command, path: &str, missing: &mut Vec<String>) {
        for arg in command.get_arguments().filter(|arg| !arg.is_hide_set()) {
            if arg
                .get_help()
                .is_none_or(|help| help.to_string().trim().is_empty())
            {
                missing.push(format!("{path}: {}", arg.get_id()));
            }
        }
        for child in command
            .get_subcommands()
            .filter(|child| !child.is_hide_set() && child.get_name() != "help")
        {
            visit(child, &format!("{path} {}", child.get_name()), missing);
        }
    }
    let mut missing = Vec::new();
    for daemon in [false, true] {
        let mut command = schema(daemon);
        command.build();
        visit(
            &command,
            if daemon { "hive-daemon" } else { "hivectl" },
            &mut missing,
        );
    }
    assert!(missing.is_empty(), "Missing argument help: {missing:#?}");
}

#[test]
fn examples_parse_and_appear_in_the_product_reference() {
    let reference = include_str!("../../../docs/cli-reference.md");
    for (path, examples) in catalog::COMMANDS {
        for example in *examples {
            assert!(
                reference.contains(example),
                "missing guide example: {example}"
            );
            let args: Vec<_> = example.split_whitespace().map(OsString::from).collect();
            if let Err(exit) = parse(&args, *path == "hive-daemon") {
                assert_eq!(exit.code, 0, "invalid example {example}: {}", exit.stderr);
            }
        }
    }
}

#[test]
fn nested_error_uses_the_selected_commands_examples() {
    for args in [
        vec!["hivectl", "auth", "login", "--bad"],
        vec![
            "hivectl",
            "auth",
            "login",
            "codex-main",
            "--probe-timeout",
            "301",
        ],
        vec!["hivectl", "--probe-timeout", "15", "auth", "login", "--bad"],
    ] {
        let value = error(&args);
        assert_eq!(
            value["examples"],
            serde_json::json!(["hivectl auth login codex-main --browser"])
        );
    }
    let value = error(&[
        "hivectl",
        "--probe-timeout",
        "301",
        "auth",
        "login",
        "codex-main",
    ]);
    assert_eq!(value["field"], "--probe-timeout");
    assert_eq!(value["received"], "301");
}

#[test]
fn missing_commands_are_errors_but_explicit_help_and_version_succeed() {
    for args in [
        vec!["hivectl"],
        vec!["hivectl", "auth"],
        vec!["hivectl", "daemon"],
    ] {
        assert_eq!(error(&args)["code"], "missing_argument");
    }
    for args in [
        vec!["hivectl", "--help"],
        vec!["hivectl", "auth", "login", "--help"],
        vec!["hivectl", "--version"],
    ] {
        let exit = rejected(&args, false);
        assert_eq!(exit.code, 0);
        assert!(!exit.stdout.is_empty());
        assert!(exit.stderr.is_empty());
    }
}

#[test]
fn recovery_builds_inherited_global_metadata_in_a_later_selected_child() {
    let value = error(&[
        "hivectl",
        "--probe-timeout",
        "--config",
        "absent.yaml",
        "auth",
        "login",
        "codex-main",
    ]);
    assert_eq!(value["field"], "--probe-timeout");
    assert_eq!(value["expected"], "an integer from 1 to 300 seconds");
    assert_eq!(
        value["examples"],
        serde_json::json!(["hivectl auth login codex-main --browser"])
    );
}

#[test]
fn sandbox_hint_only_advertises_supported_values() {
    let value = error(&[
        "hivectl",
        "spawn",
        "--cwd",
        "/repo",
        "--body",
        "hello",
        "--sandbox",
        "danger-full-access",
    ]);
    assert_eq!(value["field"], "--sandbox");
    assert_eq!(
        value["expected"],
        "read-only, workspace-write or full-access"
    );
    for sandbox in ["read-only", "workspace-write", "full-access"] {
        assert!(
            parse(
                &argv(&[
                    "hivectl",
                    "spawn",
                    "--cwd",
                    "/repo",
                    "--body",
                    "hello",
                    "--sandbox",
                    sandbox
                ]),
                false
            )
            .is_ok()
        );
    }
}

#[test]
fn invalid_private_values_and_unmapped_argv_are_not_echoed() {
    let sentinel = "PRIVATE-PROMPT-CREDENTIAL-PATH";
    for args in [
        vec![
            "hivectl",
            "spawn",
            "--body-file",
            sentinel,
            "--provider",
            "wrong",
        ],
        vec!["hivectl", "approvals", "--after", sentinel, "--wait"],
        vec![
            "hivectl",
            "auth",
            "login",
            "codex-main",
            "--PRIVATE-PROMPT-CREDENTIAL-PATH",
        ],
        vec!["hivectl", sentinel],
    ] {
        let exit = rejected(&args, false);
        assert_eq!(exit.code, 2);
        assert!(!exit.stdout.contains(sentinel));
        assert!(!exit.stderr.contains(sentinel));
    }
    let value = error(&["hivectl", "approvals", "--after", sentinel, "--wait"]);
    assert!(value.get("received").is_none());
}

#[test]
fn repeated_field_names_use_the_selected_command_policy() {
    let send = error(&[
        "hivectl",
        "send",
        "agent",
        "--body-file",
        "hello.md",
        "--wait",
        "--timeout",
        "601",
    ]);
    assert_eq!(send["expected"], "an integer from 1 to 600 seconds");
    let wait = error(&["hivectl", "wait", "agent", "--timeout", "3601"]);
    assert_eq!(wait["expected"], "an integer from 1 to 3600 seconds");
    assert!(
        parse(
            &argv(&["hivectl", "wait", "agent", "--timeout", "3600"]),
            false
        )
        .is_ok()
    );
    let join = error(&["hivectl", "wait", "--join", "join-id", "--timeout", "601"]);
    assert_eq!(join["code"], "out_of_range");
    assert_eq!(join["expected"], "an integer from 1 to 600 seconds");
    assert!(
        parse(
            &argv(&["hivectl", "wait", "--join", "join-id", "--timeout", "600"]),
            false
        )
        .is_ok()
    );
}

#[test]
fn daemon_facade_accepts_globals_and_rejects_commands_before_dispatch() {
    let cli = match parse(
        &argv(&[
            "hive-daemon",
            "--config",
            "absent.yaml",
            "--codex-bin",
            "codex",
        ]),
        true,
    ) {
        Ok(cli) => cli,
        Err(exit) => panic!("{}", exit.stderr),
    };
    assert!(matches!(
        cli.command,
        Command::Daemon {
            command: super::super::DaemonCommand::Run
        }
    ));
    let help = rejected(&["hive-daemon", "--help"], true);
    assert_eq!(help.code, 0);
    assert!(help.stdout.contains("Usage: hive-daemon"));
    assert!(!help.stdout.contains("Commands:"));
    assert!(
        help.stdout
            .contains("hive-daemon --config examples/auth.yaml")
    );
    for args in [
        vec!["hive-daemon", "daemon", "run"],
        vec!["hive-daemon", "mcp"],
        vec!["hive-daemon", "--", "status"],
    ] {
        let exit = rejected(&args, true);
        assert_eq!(exit.code, 2);
        assert!(
            exit.stdout
                .contains("hive-daemon --config examples/auth.yaml")
        );
    }
}

#[test]
fn daemon_forwarding_preserves_accepted_delimiters_and_option_values() {
    for args in [
        vec!["hive-daemon", "--"],
        vec!["hive-daemon", "--config", "absent.yaml", "--"],
        vec!["hive-daemon", "--config=--"],
    ] {
        let cli = match parse(&argv(&args), true) {
            Ok(cli) => cli,
            Err(exit) => panic!("{args:?}: {}", exit.stderr),
        };
        assert!(matches!(
            cli.command,
            Command::Daemon {
                command: super::super::DaemonCommand::Run
            }
        ));
        let expected = match args[1] {
            "--config" => Some("absent.yaml"),
            "--config=--" => Some("--"),
            _ => None,
        };
        assert_eq!(cli.config.as_deref(), expected.map(std::path::Path::new));
    }
}

struct Closed;
impl Write for Closed {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::ErrorKind::BrokenPipe.into())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn broken_output_pipe_is_operational_and_does_not_hide_other_channel() {
    let exit = rejected(&["hivectl", "auth", "login", "--bad"], false);
    let mut human = Vec::new();
    assert_eq!(emit(&exit, &mut Closed, &mut human), 1);
    assert_eq!(human, exit.stderr.as_bytes());
    let mut output = Vec::new();
    assert_eq!(emit(&exit, &mut output, &mut Closed), 1);
    assert_eq!(output, exit.stdout.as_bytes());
    assert_eq!(emit(&exit, &mut Vec::new(), &mut Vec::new()), 2);
}
