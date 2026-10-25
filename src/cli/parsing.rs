//! Parse and render input failures before any product state or provider access.
pub(super) mod globals;
mod rules;
#[cfg(test)]
mod tests;

use super::{Cli, Command, catalog};
use clap::{CommandFactory, FromArgMatches};
use hive_input::{
    Code, InputError, Product, Rendered,
    clap::{Context, Decision},
};
use std::{ffi::OsString, io::Write};

struct Exit {
    stdout: String,
    stderr: String,
    code: i32,
}
impl Exit {
    fn operational() -> Self {
        Self {
            stdout: String::new(),
            stderr: "Hive: cannot process command-line input.\n".into(),
            code: 1,
        }
    }
    fn cause(error: &(dyn std::error::Error + 'static)) -> Self {
        let mut exit = Self::operational();
        // Parsing precedes logger/environment initialization. Only bounded
        // sanitizer output may accompany this exceptional internal failure.
        exit.stderr = format!(
            "{}Cause: {:?}\n",
            exit.stderr,
            hive_log::Causes::classify(error)
        );
        exit
    }
    fn input(error: &InputError) -> Self {
        error.render(Product::Hive).map_or_else(
            |error| Self::cause(&error),
            |Rendered {
                 stdout,
                 stderr,
                 exit_code,
             }| Self {
                stdout,
                stderr,
                code: i32::from(exit_code),
            },
        )
    }
}

pub(super) fn read(daemon: bool) -> Cli {
    read_args(&std::env::args_os().collect::<Vec<_>>(), daemon)
}
pub(super) fn read_args(args: &[OsString], daemon: bool) -> Cli {
    match parse(args, daemon) {
        Ok(cli) => cli,
        Err(exit) => std::process::exit(emit(
            &exit,
            &mut std::io::stdout().lock(),
            &mut std::io::stderr().lock(),
        )),
    }
}

/// Successful informational output uses the same checked channels as help.
pub(super) fn emit_output(stdout: String) -> i32 {
    emit(
        &Exit {
            stdout,
            stderr: String::new(),
            code: 0,
        },
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    )
}

/// Bound and escape public messages; retained causes never enter CLI output.
pub(super) fn human(error: &super::Error) -> String {
    let mut escaped = error.message().chars().flat_map(char::escape_default);
    let message: String = escaped.by_ref().take(200).collect();
    let suffix = if escaped.next().is_some() { "..." } else { "" };
    format!("Hive: {}: {message}{suffix}\n", error.code())
}

/// Ordinary JSON commands retain their payload and command outcome.
pub(super) fn emit_command(
    value: &serde_json::Value,
    code: i32,
    stderr: String,
    suppressed: bool,
) -> i32 {
    let exit = Exit {
        stdout: format!("{value:#}\n"),
        stderr,
        code,
    };
    emit_with_suppression(
        &exit,
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
        suppressed,
    )
}

pub(super) fn emit_failure_record(value: &serde_json::Value, stderr: String) -> i32 {
    emit(
        &Exit {
            stdout: format!("{value}\n"),
            stderr,
            code: 1,
        },
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    )
}

/// Never acquire stdout: a detached MCP writer may still hold its lock.
pub(super) fn emit_stderr(stderr: String) -> i32 {
    emit(
        &Exit {
            stdout: String::new(),
            stderr,
            code: 1,
        },
        &mut std::io::sink(),
        &mut std::io::stderr().lock(),
    )
}

pub(super) fn emit_file_failure(failure: &super::input::Failure) -> i32 {
    let exit = file_failure(failure);
    emit(
        &exit,
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    )
}

fn file_failure(failure: &super::input::Failure) -> Exit {
    let mut exit = match &failure.error {
        super::Error::Input { input, .. } => Exit::input(input),
        error => Exit {
            stdout: format!(
                "{}\n",
                super::api::value(super::api::ErrorResponse {
                    error: error.clone()
                })
            ),
            stderr: human(error),
            code: failure.exit_code,
        },
    };
    if let Some(path) = failure.path_line() {
        exit.stderr.push_str(&path);
    }
    exit
}

pub(super) fn emit_remote_input(
    error: &InputError,
    value: &serde_json::Value,
    path: Option<&std::path::Path>,
) -> i32 {
    let mut exit = Exit::input(error);
    // The typed command context already supplied safe CLI fields/examples and
    // preserved the daemon's allowed retry identity. Keep follow output JSONL.
    if exit.code == 2 {
        exit.stdout = format!("{value}\n");
    }
    if let Some(path) = path {
        exit.stderr
            .push_str(&super::input::Failure::render_path(path));
    }
    emit(
        &exit,
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    )
}

fn schema(daemon: bool) -> clap::Command {
    let root = Cli::command().bin_name("hivectl");
    if daemon {
        let facade = clap::Command::new("hive-daemon")
            .bin_name("hive-daemon")
            .version(crate::runtime::features::VERSION)
            .about("Run the Hive controller in the foreground")
            .args(
                root.get_arguments()
                    .filter(|arg| arg.is_global_set())
                    .cloned(),
            );
        catalog::decorate(facade, "hive-daemon")
    } else {
        catalog::decorate(root, "")
    }
}

fn parse(args: &[OsString], daemon: bool) -> Result<Cli, Exit> {
    let mut command = schema(daemon);
    let matches = command
        .try_get_matches_from_mut(args)
        .map_err(|error| failure(&command, args, daemon, &error))?;
    let matches = if daemon {
        // The globals-only facade has accepted argv. Construct the existing
        // internal daemon-run invocation only after validation/help handling.
        // Insert before accepted globals so a terminal `--` cannot turn the
        // internal command names into positional arguments. Preserve values.
        let mut internal = vec![
            OsString::from("hivectl"),
            OsString::from("daemon"),
            OsString::from("run"),
        ];
        internal.extend_from_slice(&args[1..]);
        schema(false)
            .try_get_matches_from(internal)
            .map_err(|error| Exit::cause(&error))?
    } else {
        matches
    };
    let mut cli = Cli::from_arg_matches(&matches).map_err(|error| Exit::cause(&error))?;
    validate(&cli)?;
    globals::validate(&matches, daemon)?;
    cli.explicit_provider = globals::explicit_provider(&matches);
    cli.explicit_globals = globals::explicit(&matches);
    Ok(cli)
}

fn validate(cli: &Cli) -> Result<(), Exit> {
    match &cli.command {
        Command::Spawn(args) => args.validate_options(),
        Command::SetOptions(args) => args.validate_options(),
        _ => Ok(()),
    }
    .map_err(|failure| file_failure(&failure))?;
    if let Command::Agents(args) = &cli.command {
        args.validate_cursor()
            .map_err(|failure| file_failure(&failure))?;
    }
    #[cfg(feature = "comb")]
    if let Command::Evidence(args) = &cli.command {
        args.validate_options()
            .map_err(|failure| file_failure(&failure))?;
    }
    if let Command::Read(read) = &cli.command {
        read.validate().map_err(|error| {
            conditional(
                "read",
                error.code,
                &format!("--{}", &error.field[1..]),
                error.expected,
            )
        })?;
    }
    if let Command::Wait(wait) = &cli.command
        && wait.ignored_cursor()
    {
        return Err(conditional(
            "wait",
            Code::ConflictingArguments,
            "--after",
            "--until turn-finished when --after is present",
        ));
    }
    if let Command::Wait(wait) = &cli.command
        && let Some(timeout) = wait.invalid_reply_timeout()
    {
        let examples = catalog::examples("wait").ok_or_else(Exit::operational)?;
        let error = InputError::new(
            Code::OutOfRange,
            "Invalid reply wait timeout.",
            "--timeout",
            "an integer from 1 to 600 seconds",
            examples.as_slice().to_vec(),
        )
        .map_err(|error| Exit::cause(&error))?
        .with_public_received(&timeout.to_string());
        return Err(Exit::input(&error));
    }
    if let Command::Send(send) = &cli.command
        && let Some(expected) = send.invalid_mode()
    {
        let examples = catalog::examples("send").ok_or_else(Exit::operational)?;
        let error = InputError::new(
            Code::ConflictingArguments,
            "The delivery mode conflicts with another argument.",
            "--mode",
            expected,
            examples.as_slice().to_vec(),
        )
        .map_err(|error| Exit::cause(&error))?;
        return Err(Exit::input(&error));
    }
    Ok(())
}

fn conditional(path: &str, code: Code, field: &str, expected: &str) -> Exit {
    let Some(examples) = catalog::examples(path) else {
        return Exit::operational();
    };
    InputError::new(
        code,
        "Invalid command arguments.",
        field,
        expected,
        examples.as_slice().to_vec(),
    )
    .map_or_else(|error| Exit::cause(&error), |error| Exit::input(&error))
}

fn failure(command: &clap::Command, args: &[OsString], daemon: bool, error: &clap::Error) -> Exit {
    if matches!(
        error.kind(),
        clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
    ) {
        return Exit {
            stdout: error.render().to_string(),
            stderr: String::new(),
            code: 0,
        };
    }
    // This tree MUST be fresh: enabling ignore_errors on a built root does not
    // propagate the flag to built descendants, losing deeply nested matches.
    let mut recovery = schema(daemon).ignore_errors(true);
    let recovered = recovery.try_get_matches_from_mut(args);
    let (selected, path, recovered) = match recovered {
        Ok(matches) => {
            // Recovery may reach farther than the failed parse. Use its tree,
            // whose visited children have inherited and built the global args.
            let (command, path) = selected(&recovery, &matches, daemon);
            (command, path, true)
        }
        Err(_) => (
            command,
            if daemon {
                "hive-daemon".into()
            } else {
                String::new()
            },
            false,
        ),
    };
    // Clap uses InvalidValue with an empty value for a missing option value.
    // These text options use its plain String parser, which accepts an
    // explicitly supplied empty string; this shape therefore means no value.
    // Do not apply the inference to enum/path/custom parsers that reject "".
    let literal_text = match path.as_str() {
        "spawn" | "send" | "complete" => Some(("body", "--body")),
        #[cfg(feature = "comb")]
        "evidence" => Some(("summary", "--summary")),
        _ => None,
    };
    if let Some((id, flag)) = literal_text
        && error.kind() == clap::error::ErrorKind::InvalidValue
        && matches!(error.get(clap::error::ContextKind::InvalidValue),
            Some(clap::error::ContextValue::String(value)) if value.is_empty())
        && selected.get_arguments().any(|arg| {
            arg.get_id() == id
                && matches!(error.get(clap::error::ContextKind::InvalidArg),
                Some(clap::error::ContextValue::String(value)) if *value == arg.to_string())
        })
    {
        return conditional(
            &path,
            Code::MissingArgument,
            flag,
            "one literal text argument",
        );
    }
    let Some(examples) = catalog::examples(&path).or_else(|| catalog::examples("")) else {
        return Exit::operational();
    };
    let rules = if recovered {
        rules::for_command(selected, &path)
    } else {
        Vec::new()
    };
    let context = match Context::new(selected, &examples, &rules) {
        Ok(context) => context,
        Err(cause) => return Exit::cause(&cause),
    };
    match context.convert(error) {
        Ok(Decision::Input(error)) => Exit::input(&error),
        // Display cases were handled before recovery; every other failure is
        // operational. Never print an unclassified raw error or parser source.
        Ok(Decision::Help | Decision::Version | Decision::Operational) => Exit::operational(),
        Err(cause) => Exit::cause(&cause),
    }
}

fn selected<'a>(
    command: &'a clap::Command,
    matches: &clap::ArgMatches,
    daemon: bool,
) -> (&'a clap::Command, String) {
    let mut command = command;
    let mut matches = matches;
    let mut names = Vec::new();
    while let Some((name, child)) = matches.subcommand() {
        let Some(definition) = command.find_subcommand(name) else {
            break;
        };
        names.push(definition.get_name());
        command = definition;
        matches = child;
    }
    (
        command,
        if daemon {
            "hive-daemon".into()
        } else {
            names.join(" ")
        },
    )
}

fn emit(exit: &Exit, stdout: &mut impl Write, stderr: &mut impl Write) -> i32 {
    emit_with_suppression(exit, stdout, stderr, false)
}

fn emit_with_suppression(
    exit: &Exit,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
    suppressed: bool,
) -> i32 {
    // Attempt both channels independently; never retry partial or failed output.
    let output = channel(stdout, &exit.stdout);
    let human = channel(stderr, &exit.stderr);
    if suppressed {
        // A recorded permanent service failure must not trigger restart loops,
        // including when the service's diagnostic output cannot be written.
        0
    } else if output.is_err() || human.is_err() {
        1
    } else {
        exit.code
    }
}

fn channel(writer: &mut impl Write, text: &str) -> std::io::Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    writer
        .write_all(text.as_bytes())
        .and_then(|()| writer.flush())
}
