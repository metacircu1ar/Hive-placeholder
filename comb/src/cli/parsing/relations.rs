//! Input relationships and explicit service PATHs, before managed setup or sockets.
use super::{catalog, operational};
use crate::cli::args::{Cli, CredentialCommand, RawCommand};
use comb::{search::Mode, values::Timestamp};
use hive_input::{Code, InputError, Product, Rendered};

pub(super) fn validate(cli: &Cli, path: &str) -> Result<(), Rendered> {
    service_path(cli, path)?;
    if let RawCommand::Service { command, logging } = &cli.command
        && !matches!(
            command,
            hive_setup_support::service::Command::Install { .. }
        )
        && let Some(field) = logging.explicit()
    {
        return Err(rejected(
            path,
            Code::ConflictingArguments,
            field,
            "logging options only with service install",
        ));
    }
    match &cli.command {
        RawCommand::Init { dir: Some(_) } if cli.comb.is_some() => {
            return Err(rejected(
                path,
                Code::ConflictingArguments,
                "DIR",
                "either init DIR or --root DIR init",
            ));
        }
        RawCommand::Consolidate(_) if cli.run.is_none() => {
            return Err(rejected(
                path,
                Code::MissingArgument,
                "--run",
                "an explicit run name for consolidation",
            ));
        }
        RawCommand::Search(args) => {
            if args.mode == Mode::Text && args.embedding_config.is_some() {
                return Err(rejected(
                    path,
                    Code::ConflictingArguments,
                    "--embedding-config",
                    "embedding configuration only with --mode hybrid or --mode vector",
                ));
            }
            let parse = |text: &Option<String>, field| {
                text.as_deref()
                    .map(str::parse::<Timestamp>)
                    .transpose()
                    .map_err(|_| rejected(path, Code::InvalidValue, field, "an RFC3339 timestamp"))
            };
            let after = parse(&args.after, "--after")?;
            let before = parse(&args.before, "--before")?;
            comb_protocol::search::date_range(after.as_ref(), before.as_ref()).map_err(|_| {
                rejected(
                    path,
                    Code::ConflictingArguments,
                    "--after",
                    "a timestamp at or before --before (compared as instants)",
                )
            })?;
        }
        RawCommand::Credential {
            command: CredentialCommand::Create { reviewer_role, .. },
        } => {
            roles(reviewer_role, path)?;
        }
        RawCommand::Setup(options) => options
            .validate_input()
            .map_err(|error| crate::cli::input::Failure::remote(error, "setup", None).render())?,
        _ => {}
    }
    Ok(())
}

fn roles(roles: &[String], path: &str) -> Result<(), Rendered> {
    comb_protocol::service::auth::validate_reviewer_roles(roles).map_err(|_| {
        rejected(
            path,
            Code::InvalidValue,
            "--reviewer-role",
            "at most 64 unique role names, each 1..512 UTF-8 bytes without control characters",
        )
    })
}

pub(super) fn rejected(path: &str, code: Code, field: &str, expected: &str) -> Rendered {
    let Some(examples) = catalog::examples(path) else {
        return operational();
    };
    let Ok(error) = InputError::new(
        code,
        "Invalid argument combination or value.",
        field,
        expected,
        examples.as_slice().to_vec(),
    ) else {
        return operational();
    };
    error
        .render(Product::Comb)
        .unwrap_or_else(|_| operational())
}

fn service_path(cli: &Cli, path: &str) -> Result<(), Rendered> {
    let service_path = match &cli.command {
        RawCommand::Setup(options) => options.service_path.as_deref(),
        RawCommand::Service {
            command: hive_setup_support::service::Command::Install { service_path, .. },
            ..
        } => service_path.as_deref(),
        _ => None,
    };
    if let Some(value) = service_path {
        hive_setup_support::environment::service_path(Some(value)).map_err(|_| {
            rejected(
                path,
                Code::InvalidValue,
                "--service-path",
                "a colon-separated list of absolute existing directories without control \
                characters or empty entries",
            )
        })?;
    }
    Ok(())
}
