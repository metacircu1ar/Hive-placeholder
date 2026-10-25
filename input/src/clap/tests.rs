#![allow(clippy::unwrap_used)]
use super::*;
use crate::Product;
use ::clap::{ArgAction, builder::ValueParser};
use serde_json::Value;
use std::ffi::OsString;

fn examples() -> Examples {
    Examples::new(vec!["hivectl spawn --model sonnet --effort low".into()]).unwrap()
}

fn schema() -> Command {
    Command::new("hivectl")
        .version("test")
        .arg(Arg::new("model").long("model").value_parser(parsers::model))
        .arg(
            Arg::new("effort")
                .long("effort")
                .value_parser(["low", "high"]),
        )
        .arg(
            Arg::new("secret")
                .long("secret")
                .value_parser(parsers::model),
        )
        .arg(
            Arg::new("yes")
                .long("yes")
                .action(ArgAction::SetTrue)
                .conflicts_with("no"),
        )
        .arg(Arg::new("no").long("no").action(ArgAction::SetTrue))
}

fn input(command: &mut Command, args: &[&str], rules: &[FieldRule<'_>]) -> InputError {
    let error = command.try_get_matches_from_mut(args).unwrap_err();
    let examples = examples();
    match Context::new(command, &examples, rules)
        .unwrap()
        .convert(&error)
        .unwrap()
    {
        Decision::Input(input) => input,
        other => panic!("expected input rejection, got {other:?}"),
    }
}

fn json(error: &InputError) -> Value {
    serde_json::to_value(error.wire()).unwrap()
}

#[test]
fn shared_parser_keeps_specific_code_and_bounds_received() {
    let mut command = schema();
    let rules = [FieldRule {
        id: "model",
        expected: "a provider model",
        received: Received::Public,
    }];
    let error = input(&mut command, &["hivectl", "--model", "a b"], &rules);
    assert_eq!(error.code(), Code::InvalidModel);
    assert_eq!(json(&error)["field"], "--model");
    assert!(
        json(&error)["expected"]
            .as_str()
            .unwrap()
            .contains("1..128")
    );
    assert_eq!(error.received.as_deref(), Some("a b"));
    let long = "x ".repeat(200);
    let error = input(&mut command, &["hivectl", "--model", &long], &rules);
    assert!(error.received.unwrap().len() <= 200);
}

#[test]
fn sensitive_and_unregistered_fields_never_echo_values() {
    let rules = [FieldRule {
        id: "secret",
        expected: "a valid credential",
        received: Received::Omit,
    }];
    for fields in [&rules[..], &[]] {
        let error = input(
            &mut schema(),
            &["hivectl", "--secret", "PRIVATE-CANARY\n"],
            fields,
        );
        for product in [Product::Hive, Product::Comb] {
            let rendered = error.render(product).unwrap();
            assert!(!rendered.stdout.contains("PRIVATE-CANARY"));
            assert!(!rendered.stderr.contains("PRIVATE-CANARY"));
        }
    }
}

#[test]
fn unknown_tokens_not_reinterpreted_as_schema_ids_or_echoed() {
    let rules = [FieldRule {
        id: "model",
        expected: "a model",
        received: Received::Public,
    }];
    for unknown in ["model", "--PRIVATE-CANARY=value", "PRIVATE-CANARY"] {
        let error = input(&mut schema(), &["hivectl", unknown], &rules);
        assert_eq!(error.code(), Code::UnknownArgument);
        assert_eq!(json(&error)["field"], "arguments");
        assert!(error.received.is_none());
        assert!(
            !error
                .render(Product::Hive)
                .unwrap()
                .stdout
                .contains("PRIVATE-CANARY")
        );
    }
}

#[test]
fn enum_and_subcommand_suggestions_must_belong_to_schema() {
    let error = input(&mut schema(), &["hivectl", "--effort", "hig"], &[]);
    assert!(json(&error)["message"].as_str().unwrap().contains("'high'"));
    assert_eq!(json(&error)["expected"], "one of: low, high");
    let mut command = Command::new("hivectl").subcommand(Command::new("status"));
    let error = input(&mut command, &["hivectl", "statu"], &[]);
    assert!(
        json(&error)["message"]
            .as_str()
            .unwrap()
            .contains("'status'")
    );
    assert_eq!(json(&error)["field"], "subcommand");

    let mut command = schema();
    let mut parser_error = command
        .try_get_matches_from_mut(["hivectl", "--effort", "bad"])
        .unwrap_err();
    for kind in [
        ContextKind::SuggestedValue,
        ContextKind::SuggestedArg,
        ContextKind::SuggestedSubcommand,
        ContextKind::SuggestedCommand,
        ContextKind::Custom,
        ContextKind::Usage,
    ] {
        parser_error.insert(kind, ContextValue::String("PRIVATE-CANARY".into()));
    }
    let Decision::Input(error) = Context::new(&command, &examples(), &[])
        .unwrap()
        .convert(&parser_error)
        .unwrap()
    else {
        panic!("input")
    };
    assert!(
        !error
            .render(Product::Hive)
            .unwrap()
            .stdout
            .contains("PRIVATE-CANARY")
    );
    assert!(
        !error
            .render(Product::Hive)
            .unwrap()
            .stderr
            .contains("PRIVATE-CANARY")
    );
}

#[test]
fn required_flag_and_conflict_use_canonical_names() {
    let mut command = schema().arg(Arg::new("request_id").long("request-id").required(true));
    let error = input(&mut command, &["hivectl"], &[]);
    assert_eq!(error.code(), Code::MissingArgument);
    assert_eq!(json(&error)["field"], "--request-id");
    let error = input(&mut schema(), &["hivectl", "--yes", "--no"], &[]);
    assert_eq!(error.code(), Code::ConflictingArguments);
    assert_eq!(json(&error)["field"], "--yes");
    assert_eq!(
        json(&error)["expected"],
        "do not combine this argument with --no"
    );
}

#[test]
fn ambiguous_positional_display_never_uses_the_public_fields_rule() {
    let mut command = Command::new("hivectl")
        .arg(Arg::new("public").value_name("VALUE").index(1))
        .arg(
            Arg::new("secret")
                .value_name("VALUE")
                .index(2)
                .value_parser(parsers::model),
        );
    let rules = [FieldRule {
        id: "public",
        expected: "public text",
        received: Received::Public,
    }];
    let error = input(
        &mut command,
        &["hivectl", "public", "PRIVATE-CANARY\n"],
        &rules,
    );
    assert_eq!(json(&error)["field"], "arguments");
    assert!(error.received.is_none());
    let output = error.render(Product::Hive).unwrap();
    assert!(!output.stdout.contains("PRIVATE-CANARY"));
    assert!(!output.stderr.contains("PRIVATE-CANARY"));
}

#[test]
fn positional_and_nested_schema_context_are_not_guessed() {
    let mut command = Command::new("hivectl").subcommand(Command::new("auth").subcommand(
        Command::new("login").arg(Arg::new("profile").value_name("PROFILE").required(true)),
    ));
    let error = command
        .try_get_matches_from_mut(["hivectl", "auth", "login"])
        .unwrap_err();
    let selected = command
        .find_subcommand("auth")
        .unwrap()
        .find_subcommand("login")
        .unwrap();
    let examples = Examples::new(vec!["hivectl auth login codex-main".into()]).unwrap();
    let Decision::Input(error) = Context::new(selected, &examples, &[])
        .unwrap()
        .convert(&error)
        .unwrap()
    else {
        panic!("input")
    };
    assert_eq!(json(&error)["field"], "PROFILE");
    assert_eq!(json(&error)["examples"][0], "hivectl auth login codex-main");
}

#[test]
fn missing_input_help_is_exit_two_at_root_and_nested_commands() {
    for help_on_missing in [false, true] {
        for nested in [false, true] {
            let make = |name| {
                Command::new(name)
                    .arg_required_else_help(help_on_missing)
                    .subcommand_required(!help_on_missing)
                    .subcommand(Command::new("status"))
            };
            let mut command = if nested {
                Command::new("hivectl").subcommand(make("auth"))
            } else {
                make("hivectl")
            };
            let argv = if nested {
                vec!["hivectl", "auth"]
            } else {
                vec!["hivectl"]
            };
            let parser_error = command.try_get_matches_from_mut(argv).unwrap_err();
            assert_eq!(
                parser_error.kind(),
                if help_on_missing {
                    ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
                } else {
                    ErrorKind::MissingSubcommand
                }
            );
            let selected = if nested {
                command.find_subcommand("auth").unwrap()
            } else {
                &command
            };
            let examples = Examples::new(vec![
                if nested {
                    "hivectl auth status"
                } else {
                    "hivectl status"
                }
                .into(),
            ])
            .unwrap();
            let Decision::Input(error) = Context::new(selected, &examples, &[])
                .unwrap()
                .convert(&parser_error)
                .unwrap()
            else {
                panic!("input")
            };
            assert_eq!(error.code(), Code::MissingArgument);
            assert_eq!(json(&error)["field"], "subcommand");
            let rendered = error.render(Product::Hive).unwrap();
            assert_eq!(rendered.exit_code, 2);
            assert!(rendered.stdout.starts_with("{\"error\":"));
            assert!(rendered.stderr.contains(&examples.help()));
        }
    }
}

#[test]
fn help_version_and_operational_errors_are_separate() {
    let examples = examples();
    for (flag, is_help) in [("--help", true), ("--version", false)] {
        let mut command = schema();
        let error = command
            .try_get_matches_from_mut(["hivectl", flag])
            .unwrap_err();
        let decision = Context::new(&command, &examples, &[])
            .unwrap()
            .convert(&error)
            .unwrap();
        assert!(matches!(
            (decision, is_help),
            (Decision::Help, true) | (Decision::Version, false)
        ));
    }
    let command = schema();
    for kind in [ErrorKind::Io, ErrorKind::Format] {
        let error = Error::raw(kind, "PRIVATE-CANARY");
        assert!(matches!(
            Context::new(&command, &examples, &[])
                .unwrap()
                .convert(&error)
                .unwrap(),
            Decision::Operational
        ));
    }
}

#[test]
fn raw_custom_error_source_is_never_forwarded() {
    let mut command = Command::new("hivectl").arg(Arg::new("input").long("input").value_parser(
        |_: &str| -> Result<String, std::io::Error> {
            Err(std::io::Error::other("PRIVATE-SOURCE"))
        },
    ));
    let error = input(&mut command, &["hivectl", "--input", "PRIVATE-VALUE"], &[]);
    assert_eq!(error.code(), Code::InvalidValue);
    let rendered = error.render(Product::Hive).unwrap();
    assert!(!rendered.stdout.contains("PRIVATE"));
    assert!(!rendered.stderr.contains("PRIVATE"));
}

#[test]
fn actual_value_count_errors_keep_safe_counts() {
    let mut command = Command::new("hivectl").arg(Arg::new("pair").long("pair").num_args(2));
    let error = input(&mut command, &["hivectl", "--pair", "one"], &[]);
    assert_eq!(json(&error)["field"], "--pair");
    assert_eq!(json(&error)["expected"], "exactly 2 values");
}

#[test]
fn field_rules_require_unique_existing_ids_and_safe_expectations() {
    let command = schema();
    let examples = examples();
    let rule = || FieldRule {
        id: "model",
        expected: "a model",
        received: Received::Public,
    };
    assert!(Context::new(&command, &examples, &[rule(), rule()]).is_err());
    assert!(
        Context::new(
            &command,
            &examples,
            &[FieldRule {
                id: "missing",
                ..rule()
            }]
        )
        .is_err()
    );
    assert!(
        Context::new(
            &command,
            &examples,
            &[FieldRule {
                expected: "private\nline",
                ..rule()
            }]
        )
        .is_err()
    );
}

#[cfg(unix)]
#[test]
fn invalid_utf8_is_input_without_lossy_value_echo() {
    use std::os::unix::ffi::OsStringExt;
    let mut command = schema();
    let error = command
        .try_get_matches_from_mut([
            OsString::from("hivectl"),
            "--effort".into(),
            OsString::from_vec(vec![0xff]),
        ])
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidUtf8);
    let Decision::Input(input) = Context::new(&command, &examples(), &[])
        .unwrap()
        .convert(&error)
        .unwrap()
    else {
        panic!("input")
    };
    assert_eq!(input.code(), Code::InvalidValue);
    assert!(input.received.is_none());
}

#[test]
fn adapters_use_same_value_rules_as_servers() {
    let parsers: Vec<(ValueParser, &str, Code)> = vec![
        (parsers::request_id.into(), "", Code::InvalidRequestId),
        (parsers::model.into(), "a b", Code::InvalidModel),
        (parsers::effort.into(), "unknown", Code::InvalidEffort),
        (parsers::run_name(3).into(), "long", Code::InvalidRunName),
        (
            parsers::duration(1..=604_800).into(),
            "8d",
            Code::InvalidDuration,
        ),
        (parsers::unsigned(1..=100).into(), "101", Code::OutOfRange),
        (parsers::count(1..=100).into(), "101", Code::OutOfRange),
        (crate::validate::uuid.into(), "invalid", Code::InvalidUuid),
    ];
    for (parser, value, code) in parsers {
        let mut command =
            Command::new("hivectl").arg(Arg::new("value").long("value").value_parser(parser));
        assert_eq!(
            input(&mut command, &["hivectl", "--value", value], &[]).code(),
            code
        );
    }
}

#[test]
fn examples_feed_after_help_and_both_error_outputs_identically() {
    let examples = examples();
    let mut command = schema().after_help(examples.help());
    assert!(
        command
            .render_long_help()
            .to_string()
            .contains(&examples.as_slice()[0])
    );
    let error = input(&mut command, &["hivectl", "--bad"], &[]);
    let output = error.render(Product::Hive).unwrap();
    assert!(output.stderr.ends_with(&examples.help()));
    assert_eq!(json(&error)["examples"][0], examples.as_slice()[0]);
}

#[test]
fn missing_input_help_names_the_single_required_positional() {
    let mut command = Command::new("hivectl")
        .arg_required_else_help(true)
        .arg(Arg::new("target").value_name("TARGET").required(true));
    let error = input(&mut command, &["hivectl"], &[]);
    assert_eq!(error.code(), Code::MissingArgument);
    assert_eq!(json(&error)["field"], "TARGET");
}

#[test]
fn platform_counts_reject_signs_whitespace_and_overflow_without_clamping() {
    let count = parsers::count(0..=usize::MAX);
    assert_eq!(count("0").unwrap(), 0);
    assert_eq!(count(&usize::MAX.to_string()).unwrap(), usize::MAX);
    for text in ["", " 1", "+1", "-1", "1.0", "184467440737095516160"] {
        assert!(count(text).is_err());
    }
}
