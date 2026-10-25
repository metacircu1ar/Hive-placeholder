//! User-service adapter; manager subprocesses use `Hive`'s guarded fork path.
mod start;
use super::{Cli, CommandOutcome, Config, Error, Path, Result, api, runtime};
use hive_setup_support::{
    files, managed,
    runner::SystemRunner,
    service::{self, Controller, Layout, Scope, Spec},
};
use std::time::{Duration, Instant};

pub(super) fn error(e: std::io::Error) -> Error {
    let error = if e.kind() == std::io::ErrorKind::InvalidInput {
        Error::ConfigInvalid {
            message: e.to_string().into(),
            source: None,
        }
    } else {
        Error::StateIo {
            message: e.to_string().into(),
            source: None,
        }
    };
    error.with_source(e)
}
pub(super) fn scope(config: &Path, root: &Path) -> Result<Scope> {
    Scope::new("hive", &[config, root]).map_err(error)
}
#[derive(serde::Deserialize)]
struct Health {
    status: String,
    #[serde(default)]
    managed_service: Option<String>,
}
pub(super) fn health(root: &Path, label: Option<&str>) -> std::io::Result<bool> {
    let rt = crate::calls::executor().map_err(std::io::Error::other)?;
    let listening = rt.block_on(async {
        match tokio::time::timeout(
            HEALTH_TIMEOUT,
            tokio::net::UnixStream::connect(runtime::client::socket_path(root)),
        )
        .await
        {
            Ok(Ok(_)) => Ok(true),
            Ok(Err(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                Ok(false)
            }
            Ok(Err(e)) => Err(e),
            Err(cause) => Err(std::io::Error::new(std::io::ErrorKind::TimedOut, cause)),
        }
    })?;
    if !listening {
        return Ok(false);
    }
    let result = rt.block_on(async {
        runtime::client::Client::new(root)
            .observe::<Health>(
                "service-health",
                &super::Request::Ping,
                tokio::time::Instant::now() + HEALTH_TIMEOUT,
            )
            .await
    });
    match result {
        Ok(h) => {
            if label.is_some() && h.managed_service.as_deref() != label {
                return Err(hive_setup_support::invalid(
                    "An unmanaged daemon is serving this state directory; stop it first.",
                ));
            }
            // Version compatibility belongs to doctor; an old running service
            // still needs stop/restart during an upgrade or root mutation.
            Ok(h.status == "running")
        }
        Err(e) if e.code() == crate::error::ErrorCode::DaemonUnavailable => Ok(false),
        Err(e) => Err(std::io::Error::other(e.message().to_owned())),
    }
}
pub(super) fn run(
    cli: &Cli,
    config: &Path,
    root: &Path,
    command: &service::Command,
) -> Result<CommandOutcome> {
    validate_path(command)?;
    run_inner(cli, config, root, command, false)
}
pub(super) fn run_new(
    cli: &Cli,
    config: &Path,
    root: &Path,
    command: &service::Command,
) -> Result<CommandOutcome> {
    validate_path(command)?;
    run_inner(cli, config, root, command, true)
}
fn run_inner(
    cli: &Cli,
    config: &Path,
    root: &Path,
    command: &service::Command,
    new: bool,
) -> Result<CommandOutcome> {
    let layout = Layout::discover().map_err(error)?;
    let scope = scope(config, root)?;
    let runner = SystemRunner(crate::os::spawn);
    let controller = Controller {
        layout: &layout,
        scope: &scope,
        runner: &runner,
    };
    let label = scope.label().map_err(error)?;
    let require_owned = !matches!(command, service::Command::Status)
        || service::load(&layout, &scope).map_err(error)?.is_some();
    let health = || health(root, require_owned.then_some(label.as_str()));
    match command {
        service::Command::Install {
            environment_file,
            program,
            service_path,
        } => {
            let state = controller.status(&health).map_err(error)?;
            if !state.manager_available {
                return Ok(CommandOutcome::ok(api::value(state)));
            }
            let spec = installation(
                cli,
                config,
                root,
                &layout,
                &scope,
                InstallOptions {
                    environment_file: environment_file.as_deref(),
                    program: program.as_deref(),
                    service_path: service_path.as_deref(),
                },
            )?;
            if new {
                controller.install_new(&spec)
            } else {
                controller.install(&spec)
            }
            .map_err(error)?;
        }
        service::Command::Start => {
            if let Some(spec) = service::load(&layout, &scope).map_err(error)? {
                validate_current(&spec)?;
            }
            if let Some(outcome) = start::run(cli, config, root, &label, &controller, &health)? {
                return Ok(outcome);
            }
        }
        service::Command::Stop => controller.stop(&health).map_err(error)?,
        service::Command::Uninstall => controller.uninstall(&health).map_err(error)?,
        service::Command::Status => (),
    }
    report(config, root)
}
#[derive(serde::Serialize)]
struct Report {
    #[serde(flatten)]
    service: service::Status,
    loaded_agents: Option<usize>,
    active_turns: Option<usize>,
    daemon: Option<super::setup::daemon::Observation>,
}

fn report(config: &Path, root: &Path) -> Result<CommandOutcome> {
    let observation = super::setup::daemon::observe(root)?;
    Ok(CommandOutcome::ok(api::value(Report {
        service: status(config, root)?,
        loaded_agents: observation.as_ref().map(|o| o.resident_actors),
        active_turns: observation
            .as_ref()
            .map(|o| o.runtime_resources.active_turns),
        daemon: observation,
    })))
}
pub(super) fn restart(cli: &Cli, config: &Path, root: &Path) -> Result<CommandOutcome> {
    start::overrides(cli)?;
    let _configuration = runtime::daemon_configuration(config)?;
    let layout = Layout::discover().map_err(error)?;
    let scope = scope(config, root)?;
    let runner = SystemRunner(crate::os::spawn);
    let controller = Controller {
        layout: &layout,
        scope: &scope,
        runner: &runner,
    };
    let _admission = crate::state::State::new(root.to_owned())?.admission()?;
    let held = controller.hold().map_err(error)?;
    validate_current(held.spec())?;
    let label = scope.label().map_err(error)?;
    let expected = start::expectation(config, label.clone())?.program(&held.spec().program)?;
    let outcome = restart_held(
        &held,
        &runner,
        root,
        Instant::now() + super::DAEMON_STOP_TIMEOUT,
        &|| health(root, Some(&label)),
        || start::checked(&expected, root, |probe| held.start(&runner, probe)),
    )?;
    if let Some(outcome) = outcome {
        return Ok(outcome);
    }
    drop(held);
    report(config, root)
}

fn restart_held<T>(
    held: &service::Held,
    runner: &dyn hive_setup_support::runner::Runner,
    root: &Path,
    deadline: Instant,
    health: &dyn Fn() -> std::io::Result<bool>,
    start: impl FnOnce() -> Result<T>,
) -> Result<T> {
    held.stop(runner, health).map_err(error)?;
    super::daemon::restart::cleanup(root, deadline)?;
    start()
}

pub(super) fn owned(config: &Path, root: &Path) -> Result<bool> {
    let layout = Layout::discover().map_err(error)?;
    Ok(service::load(&layout, &scope(config, root)?)
        .map_err(error)?
        .is_some())
}
pub(super) fn status(config: &Path, root: &Path) -> Result<service::Status> {
    let layout = Layout::discover().map_err(error)?;
    let scope = scope(config, root)?;
    let label = scope.label().map_err(error)?;
    let runner = SystemRunner(crate::os::spawn);
    let mut status = Controller {
        layout: &layout,
        scope: &scope,
        runner: &runner,
    }
    .status(&|| {
        health(
            root,
            if service::load(&layout, &scope)?.is_some() {
                Some(&label)
            } else {
                None
            },
        )
    })
    .map_err(error)?;
    if let Some(spec) = service::load(&layout, &scope).map_err(error)?
        && let Some(config) =
            crate::observability::optional(Config::load(config), "service_status_config")
    {
        status.missing_environment.extend(
            hive_setup_support::environment::missing(
                spec.environment_file.as_deref(),
                &config
                    .auth_profiles
                    .values()
                    .filter_map(|p| p.api_key_env.clone())
                    .collect::<Vec<_>>(),
            )
            .map_err(error)?,
        );
        status.missing_environment.sort();
        status.missing_environment.dedup();
    }
    if let Some(spec) = service::load(&layout, &scope).map_err(error)? {
        let path = spec.environment.get("PATH").map_or("", String::as_str);
        for args in spec.arguments.windows(2) {
            if matches!(args[0].as_str(), "--codex-bin" | "--claude-bin")
                && let Some(warning) =
                    hive_setup_support::environment::interpreter_warning(Path::new(&args[1]), path)
                        .map_err(error)?
            {
                status.warnings.push(warning);
            }
        }
    }
    Ok(status)
}
pub(super) fn validate_current(spec: &Spec) -> Result<()> {
    let config = spec
        .scope
        .paths
        .first()
        .ok_or_else(|| Error::invalid_path("Missing service configuration path."))?;
    let config = Config::load(config)?;
    let names: Vec<_> = config
        .auth_profiles
        .values()
        .filter_map(|p| p.api_key_env.clone())
        .collect();
    let missing =
        hive_setup_support::environment::missing(spec.environment_file.as_deref(), &names)
            .map_err(error)?;
    if !missing.is_empty() {
        return Err(error(hive_setup_support::invalid(format!(
            "Service environment file is missing required names: {}",
            missing.join(", ")
        ))));
    }
    Ok(())
}
pub(super) fn prepare(cli: &Cli) -> Result<Option<Spec>> {
    let Some(path) = &cli.managed_service else {
        return Ok(None);
    };
    if !matches!(
        cli.command,
        super::Command::Daemon {
            command: super::DaemonCommand::Run
        }
    ) {
        return Err(Error::invalid_request(
            "Managed execution is only valid for daemon run.",
        ));
    }
    let config = cli.config_path()?;
    let spec = managed::enter(path, &scope(&config, &cli.state_root()?)?).map_err(error)?;
    Ok(Some(spec))
}
pub(super) fn suppress(spec: Option<&Spec>, error: &Error) -> bool {
    let permanent = matches!(error.code(), "config_invalid" | "invalid_path");
    spec.is_some_and(|spec| {
        managed::startup_failed(spec, error.code(), permanent)
            .inspect_err(|cause| {
                if let Err(diagnostic) =
                    super::logging::service_failure("service_suppression", cause)
                {
                    // No diagnostic sink could be started. Only a fixed final
                    // notice is safe here, and the original failure still wins.
                    eprintln!(
                        "Hive: service failure recording and diagnostic setup failed; errno={}.",
                        diagnostic.raw_os_error().unwrap_or(0)
                    );
                }
            })
            .unwrap_or(false)
    })
}

fn config_keys(config: &Config) -> impl Iterator<Item = String> + '_ {
    config
        .auth_profiles
        .values()
        .filter_map(|p| p.api_key_env.clone())
}

fn provider_arguments(cli: &Cli, arguments: &mut Vec<String>) -> Result<()> {
    for (flag, bin) in [
        ("--codex-bin", &cli.codex_bin),
        ("--claude-bin", &cli.claude_bin),
    ] {
        // Resolve available providers now; absent optional providers retain their names.
        let path = crate::observability::optional(
            files::executable(Path::new(bin)),
            "service_optional_binary",
        )
        .unwrap_or_else(|| bin.into());
        arguments.extend([flag.into(), service::path_text(&path).map_err(error)?]);
    }
    Ok(())
}

fn validate_path(command: &service::Command) -> Result<()> {
    if let service::Command::Install {
        service_path: Some(path),
        ..
    } = command
    {
        hive_setup_support::environment::service_path(Some(path)).map_err(|cause| {
            super::input::Context::new("service install", "--service-path")
                .invalid(
                    hive_input::Code::InvalidValue,
                    "Invalid service search path.",
                    "a colon-separated list of absolute existing directories without control \
                        characters or empty entries",
                )
                .with_source(cause)
                .error
        })?;
    }
    Ok(())
}

struct InstallOptions<'a> {
    environment_file: Option<&'a Path>,
    program: Option<&'a Path>,
    service_path: Option<&'a str>,
}
fn installation(
    cli: &Cli,
    config: &Path,
    root: &Path,
    layout: &Layout,
    scope: &Scope,
    options: InstallOptions<'_>,
) -> Result<Spec> {
    let InstallOptions {
        environment_file,
        program,
        service_path,
    } = options;
    let config_data = Config::load(config)?;
    let program = program
        .map(Path::to_owned)
        .or_else(|| std::env::args_os().next().map(Into::into))
        .ok_or_else(|| Error::invalid_path("Cannot find invoked program."))?;
    let mut arguments = vec![
        "--config".into(),
        files::location(config)
            .and_then(|p| service::path_text(&p))
            .map_err(error)?,
        "--state-dir".into(),
        files::location(root)
            .and_then(|p| service::path_text(&p))
            .map_err(error)?,
    ];
    provider_arguments(cli, &mut arguments)?;
    if program.file_name() != Some(std::ffi::OsStr::new("hive-daemon")) {
        arguments.extend(["daemon".into(), "run".into()]);
    }
    let required = hive_setup_support::environment::required(config_keys(&config_data));
    let mut spec = Spec::new(
        layout,
        scope.clone(),
        &program,
        arguments,
        environment_file,
        required,
    )
    .map_err(error)?;
    if let Some(path) = service_path {
        spec.environment.insert(
            "PATH".into(),
            hive_setup_support::environment::service_path(Some(path)).map_err(error)?,
        );
    }
    spec.check_environment().map_err(error)?;
    spec.required_environment
        .retain(|key| hive_setup_support::environment::PROXIES.contains(&key.as_str()));
    Ok(spec)
}

const HEALTH_TIMEOUT: std::time::Duration = Duration::from_secs(2);
