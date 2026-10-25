//! Authentication commands and daemon availability reporting.
pub(super) mod selection;
use super::{
    Auth, AuthCommand, Cancellation, Cli, CommandOutcome, Config, Duration, Error, LoginOutcome,
    Path, PathBuf, Request, Result, Settings, State, api, remote_response, runtime,
};

#[derive(serde::Serialize)]
struct Generations {
    generations: Vec<crate::state::Generation>,
}

pub(super) fn auth_command(
    command: &AuthCommand,
    root: PathBuf,
    cli: &Cli,
) -> Result<CommandOutcome> {
    match command {
        AuthCommand::Groups => {
            let state = State::new(root)?;
            return remote_response(
                &state.root,
                &uuid::Uuid::new_v4().to_string(),
                Request::AuthGroups,
            );
        }
        AuthCommand::Usage { name } => {
            let state = State::new(root)?;
            return remote_response(
                &state.root,
                &uuid::Uuid::new_v4().to_string(),
                Request::Usage(api::UsageArgs {
                    auth: name.clone().map(Into::into).unwrap_or_default(),
                }),
            );
        }
        AuthCommand::HistoryExport { output } => {
            return Ok(CommandOutcome::ok(
                State::new(root)?.export_retired(output)?,
            ));
        }
        AuthCommand::Generations { name } => {
            return Ok(CommandOutcome::ok(api::value(Generations {
                generations: State::new(root)?.generations(name)?,
            })));
        }
        AuthCommand::Prune { name, generation } => {
            return Ok(CommandOutcome::ok(
                State::new(root)?.prune(name, *generation)?,
            ));
        }
        AuthCommand::List => {
            return Ok(CommandOutcome::ok(crate::auth::list_profiles(
                &Config::load(&cli.config_path()?)?,
                &State::new(root)?,
            )?));
        }
        _ => {}
    }
    let mut auth = authentication(command, &cli.config_path()?, root, cli)?;
    if let Some(outcome) = key_command(&auth, command)? {
        return Ok(outcome);
    }
    provider_command(&mut auth, command)
}

fn key_command(auth: &Auth, command: &AuthCommand) -> Result<Option<CommandOutcome>> {
    let (name, register) = match command {
        AuthCommand::Status { name } => (name, false),
        AuthCommand::Login { name, .. } => (name, true),
        _ => return Ok(None),
    };
    let profile = auth.config.profile(name)?;
    if profile.method != crate::config::Method::ApiKey {
        return Ok(None);
    }
    let request = Request::KeySource(api::KeySourceArgs {
        auth: name.clone(),
        profile: profile.clone(),
        register,
    });
    match remote_response(&auth.state.root, &uuid::Uuid::new_v4().to_string(), request) {
        Ok(mut outcome) => {
            if outcome.success {
                #[derive(serde::Deserialize)]
                struct SourceReply {
                    state: crate::auth::Status,
                    environment_source: String,
                    profile: String,
                }
                let reply: SourceReply =
                    serde_json::from_value(outcome.value.clone()).map_err(|cause| {
                        Error::daemon_protocol_error("Invalid API source status.")
                            .with_source(cause)
                    })?;
                if reply.environment_source != "daemon" || reply.profile != *name {
                    return Err(Error::daemon_protocol_error(
                        "API source observation identity differs.",
                    ));
                }
                outcome.success =
                    !register || reply.state == crate::auth::Status::PresentUnverified;
            }
            Ok(Some(outcome))
        }
        Err(error @ Error::DaemonUnavailable { .. }) if absent_socket(&error) => {
            // A refused/missing socket alone is not absence: retain the daemon's
            // ownership lock through the local observation whenever state exists.
            let _owner = match auth.state.root.symlink_metadata() {
                Ok(_) => Some(runtime::maintenance_lock(&auth.state.root)?),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(Error::state_io_from(error)),
            };
            let (report, success) = if register {
                let outcome = auth.login(name)?;
                (outcome.report, outcome.success)
            } else {
                (auth.status(name)?, true)
            };
            Ok(Some(CommandOutcome {
                value: api::value(report),
                success,
                input: None,
            }))
        }
        Err(error) => Err(error),
    }
}
fn absent_socket(error: &Error) -> bool {
    std::error::Error::source(error)
        .and_then(|cause| cause.downcast_ref::<std::io::Error>())
        .is_some_and(|cause| {
            matches!(
                cause.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            )
        })
}
pub(super) fn authentication(
    command: &AuthCommand,
    config_path: &Path,
    root: PathBuf,
    cli: &Cli,
) -> Result<Auth> {
    let config = Config::load(config_path)?;
    selection::auth(&config, command, &root)?;
    let browser = matches!(
        command,
        AuthCommand::Login { browser: true, .. } | AuthCommand::Repair { browser: true, .. }
    );
    let reauthenticate = matches!(
        command,
        AuthCommand::Login {
            reauthenticate: true,
            ..
        } | AuthCommand::Repair {
            reauthenticate: true,
            ..
        }
    );
    Ok(Auth {
        config,
        state: State::new(root)?,
        settings: Settings {
            #[cfg(not(test))]
            launcher: crate::launch::system_cache(),
            #[cfg(test)]
            launcher: crate::launch::test_cache()?,
            codex: cli.codex_bin.clone(),
            claude: cli.claude_bin.clone(),
            probe_timeout: Duration::from_secs(cli.probe_timeout),
            login_timeout: Duration::from_secs(cli.login_timeout),
            browser,
            reauthenticate,
            cancellation: Cancellation::install()?,
        },
    })
}
#[derive(serde::Deserialize)]
struct AvailabilityReply {
    availability: Option<Vec<runtime::Availability>>,
    error: Option<serde_json::Value>,
}
pub(super) fn auth_report(
    root: &Path,
    mut report: crate::auth::Report,
    success: bool,
) -> CommandOutcome {
    let request = if report.state == crate::auth::Status::Authenticated {
        Request::LoginRestored(api::AuthArgs {
            auth: report.profile.clone().into(),
        })
    } else {
        Request::AuthAvailability(api::AuthArgs {
            auth: report.profile.clone().into(),
        })
    };
    let response = runtime::client(root, &uuid::Uuid::new_v4().to_string(), request);
    let availability = match response {
        Ok(value) => match serde_json::from_value::<AvailabilityReply>(value) {
            Ok(reply) => {
                if reply.error.is_some() {
                    report.warnings.push(Error::agent_busy(
                        "Daemon did not confirm login recovery; repeat auth status to retry.",
                    ));
                }
                reply.availability
            }
            Err(cause) => {
                report.warnings.push(
                    Error::daemon_protocol_error("Invalid login availability response.")
                        .with_source(cause),
                );
                None
            }
        },
        Err(Error::DaemonUnavailable { .. }) => None,
        Err(error) => {
            report.warnings.push(error);
            None
        }
    };
    let mut value = api::value(report);
    if let Some(availability) = availability {
        value["availability"] = api::value(availability);
    }
    CommandOutcome {
        value,
        success,
        input: None,
    }
}

fn provider_command(auth: &mut Auth, command: &AuthCommand) -> Result<CommandOutcome> {
    match command {
        AuthCommand::Status { name } => {
            let report = auth.status(name)?;
            Ok(auth_report(&auth.state.root, report, true))
        }
        AuthCommand::Login { name, .. } => {
            let LoginOutcome { report, success } = auth.login(name)?;
            Ok(auth_report(&auth.state.root, report, success))
        }
        AuthCommand::Recover { name } => {
            let outcome = auth.recover(name)?;
            Ok(CommandOutcome {
                input: None,
                value: outcome.report,
                success: outcome.success,
            })
        }
        AuthCommand::Repair {
            name,
            new_state_dir,
            ..
        } => {
            let LoginOutcome { report, success } = auth.repair(name, new_state_dir.as_deref())?;
            Ok(auth_report(&auth.state.root, report, success))
        }
        AuthCommand::Groups
        | AuthCommand::Usage { .. }
        | AuthCommand::HistoryExport { .. }
        | AuthCommand::Generations { .. }
        | AuthCommand::Prune { .. }
        | AuthCommand::List => {
            unreachable!("non-provider auth commands are dispatched before provider setup")
        }
    }
}
