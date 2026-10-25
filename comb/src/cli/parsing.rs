//! Parse before touching knowledge, credentials, services or sockets.
mod applicability;
#[cfg(test)]
mod applicability_tests;
#[cfg(test)]
mod coverage;
#[cfg(test)]
mod http_tests;
#[cfg(test)]
mod model_tests;
#[cfg(test)]
mod payload_tests;
mod payloads;
#[cfg(test)]
mod preflight_tests;
mod relations;
#[cfg(test)]
mod setup_tests;
#[cfg(test)]
mod tests;
pub(super) mod values;

use super::{Cli, catalog};
use clap::{CommandFactory, FromArgMatches};
use hive_input::{
    Product, Rendered,
    clap::{Context, Decision, FieldRule, Received},
};
use std::{ffi::OsString, io::Write};

pub(crate) fn read() -> Result<Cli, Rendered> {
    parse(&std::env::args_os().collect::<Vec<_>>())
}

fn schema() -> clap::Command {
    catalog::decorate(Cli::command().bin_name("comb"), "")
}

fn parse(args: &[OsString]) -> Result<Cli, Rendered> {
    let mut command = schema();
    let matches = command
        .try_get_matches_from_mut(args)
        .map_err(|error| failure(args, &error))?;
    let cli = Cli::from_arg_matches(&matches).map_err(|_| operational())?;
    let (_, path) = selected(&command, &matches);
    relations::validate(&cli, &path)?;
    applicability::validate(&cli, &path)?;
    payloads::validate(&cli.command, &path)?;
    Ok(cli)
}

fn failure(args: &[OsString], error: &clap::Error) -> Rendered {
    if matches!(
        error.kind(),
        clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
    ) {
        return Rendered {
            stdout: error.render().to_string(),
            stderr: String::new(),
            exit_code: 0,
        };
    }
    // Recover command selection from a fresh tree: ignore_errors cannot propagate
    // into children already built by the failed parse. Never scan raw argv names.
    let mut recovery = diagnostic_tree(schema()).ignore_errors(true);
    let matches = recovery.try_get_matches_from_mut(args);
    let (command, path) = match &matches {
        Ok(matches) => selected(&recovery, matches),
        // Recovery is diagnostic-only; its failure must never replace the
        // original caller error with an operational failure or display request.
        Err(_) => (&recovery, String::new()),
    };
    let Some(examples) = catalog::examples(&path) else {
        return operational();
    };
    let rules = field_rules(command, &path);
    let Ok(context) = Context::new(command, &examples, &rules) else {
        return operational();
    };
    match context.convert(error) {
        Ok(Decision::Input(error)) => error
            .render(Product::Comb)
            .unwrap_or_else(|_| operational()),
        Ok(Decision::Help | Decision::Version | Decision::Operational) | Err(_) => operational(),
    }
}

fn diagnostic_tree(command: clap::Command) -> clap::Command {
    // After a missing option value, recovery may reach a later --help/--version.
    // Disable those handlers only here so nested metadata can still be recovered.
    command
        .disable_help_flag(true)
        .disable_version_flag(true)
        .disable_help_subcommand(true)
        .mut_subcommands(diagnostic_tree)
}

fn selected<'a>(
    command: &'a clap::Command,
    matches: &clap::ArgMatches,
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
    (command, names.join(" "))
}

fn field_rules<'a>(command: &'a clap::Command, path: &str) -> Vec<FieldRule<'a>> {
    command
        .get_arguments()
        .filter_map(|arg| {
            let id = arg.get_id().as_str();
            let expected = values::expected(path, id)?;
            let received = if matches!(
                id,
                "limit"
                    | "depth"
                    | "vector_cache_mib"
                    | "dimensions"
                    | "max_tokens"
                    | "timeout_seconds"
            ) {
                Received::Public
            } else {
                Received::Omit
            };
            Some(FieldRule {
                id,
                expected,
                received,
            })
        })
        .collect()
}

fn operational() -> Rendered {
    Rendered {
        stdout: String::new(),
        stderr: "Comb: cannot process command-line input.\n".into(),
        exit_code: 1,
    }
}

pub(crate) fn emit(output: &Rendered) -> u8 {
    emit_to(
        output,
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    )
}

pub(super) fn emit_to(output: &Rendered, stdout: &mut impl Write, stderr: &mut impl Write) -> u8 {
    let json = channel(stdout, &output.stdout);
    let human = channel(stderr, &output.stderr);
    if json.is_err() || human.is_err() {
        1
    } else {
        output.exit_code
    }
}

pub(super) fn channel(writer: &mut impl Write, text: &str) -> std::io::Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    writer
        .write_all(text.as_bytes())
        .and_then(|()| writer.flush())
}
