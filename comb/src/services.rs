//! Owned login-service orchestration; authenticated health never opens the writer.
mod setup;
#[cfg(test)]
mod tests;
use crate::{
    Error, Result, provision,
    service::{
        self, Operation,
        auth::{Connection, ConnectionExt as _},
    },
};
use hive_setup_support::{
    files,
    runner::SystemRunner,
    service::{Command, Controller, Layout, Scope, Spec, Status},
};
use std::{path::Path, time::Duration};

pub fn error(e: std::io::Error) -> Error {
    if let Some(restart) = e
        .get_ref()
        .and_then(|e| e.downcast_ref::<hive_setup_support::service::RestartFailure>())
    {
        let result = match &restart.operation {
            Some(operation) => operation
                .get_ref()
                .and_then(|e| e.downcast_ref::<Error>())
                .cloned()
                .unwrap_or_else(|| Error::service_io(operation)),
            None => Error::service_operation_committed().committed(),
        };
        return Error::service_restart_failed(result, &restart.restart).caused_by(e);
    }
    if let Some(inner) = e.get_ref().and_then(|e| e.downcast_ref::<Error>()) {
        return inner.clone();
    }
    Error::service_io(&e).caused_by(e)
}
pub fn scope(root: &Path) -> std::io::Result<Scope> {
    Scope::new("comb", &[root])
}
pub fn health(
    root: &Path,
    require_owned: bool,
) -> Result<Option<comb_protocol::service::api::Health>> {
    if !root.join(".comb").try_exists()? {
        return Ok(None);
    }
    let settings = provision::settings(root)?;
    let socket = service::socket_path(root, settings.as_ref().map(|s| s.socket.as_path()))?;
    let connection = Connection::human(root, socket)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let value = rt.block_on(service::client::try_call(
        &connection,
        &Operation::Health,
        None,
        HEALTH_TIMEOUT,
    ))?;
    let health: Option<comb_protocol::service::api::Health> =
        value.map(serde_json::from_value).transpose()?;
    if require_owned
        && let Some(health) = &health
        && health.managed_service.as_deref() != Some(scope(root)?.label()?.as_str())
    {
        return Err(Error::unmanaged_service());
    }
    Ok(health)
}
fn ready(root: &Path) -> std::io::Result<bool> {
    health(root, true)
        .map(|h| h.is_some())
        .map_err(std::io::Error::other)
}
pub fn status(root: &Path) -> Result<Status> {
    let layout = Layout::discover()?;
    let scope = scope(root)?;
    let runner = SystemRunner(std::process::Command::spawn);
    let mut state = Controller {
        layout: &layout,
        scope: &scope,
        runner: &runner,
    }
    .status(&|| {
        health(root, owned(root).map_err(std::io::Error::other)?)
            .map(|h| h.is_some())
            .map_err(std::io::Error::other)
    })
    .map_err(error)?;
    if let Some(spec) = hive_setup_support::service::load(&layout, &scope)? {
        state
            .missing_environment
            .extend(hive_setup_support::environment::missing(
                spec.environment_file.as_deref(),
                &required_environment(root)?,
            )?);
        state.missing_environment.sort();
        state.missing_environment.dedup();
    }
    Ok(state)
}
pub fn run(root: &Path, command: &Command) -> Result<Status> {
    run_configured(root, command, &hive_log::Config::default())
}
pub fn run_configured(
    root: &Path,
    command: &Command,
    logging: &hive_log::Config,
) -> Result<Status> {
    logging.validate()?;
    run_inner(root, command, false, logging)
}
fn run_new(root: &Path, command: &Command) -> Result<Status> {
    run_inner(root, command, true, &hive_log::Config::default())
}
fn run_inner(
    root: &Path,
    command: &Command,
    new: bool,
    logging: &hive_log::Config,
) -> Result<Status> {
    let layout = Layout::discover()?;
    let scope = scope(root)?;
    let runner = SystemRunner(std::process::Command::spawn);
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &runner,
    };
    match command {
        Command::Install {
            environment_file,
            program,
            service_path,
        } => {
            let state = ctl.status(&|| ready(root)).map_err(error)?;
            if !state.manager_available {
                return Ok(state);
            }
            let settings = provision::settings(root)?.ok_or_else(Error::service_setup_required)?;
            let program = program
                .clone()
                .or_else(|| std::env::args_os().next().map(Into::into))
                .ok_or_else(Error::invoked_executable_missing)?;
            let names = match settings.embeddings {
                Some(crate::embedding::Config::Http(config)) => {
                    config.api_key_env.into_iter().collect::<Vec<_>>()
                }
                _ => vec![],
            };
            let mut spec = Spec::new(
                &layout,
                scope.clone(),
                &program,
                serve_arguments(root, logging)?,
                environment_file.as_deref(),
                hive_setup_support::environment::required(names),
            )?;
            if let Some(path) = service_path {
                spec.environment.insert(
                    "PATH".into(),
                    hive_setup_support::environment::service_path(Some(path))?,
                );
            }
            spec.check_environment()?;
            spec.required_environment
                .retain(|key| hive_setup_support::environment::PROXIES.contains(&key.as_str()));
            if new {
                ctl.install_new(&spec)
            } else {
                ctl.install(&spec)
            }
            .map_err(error)?;
        }
        Command::Start => {
            validate_environment(root, &required_environment(root)?)?;
            ctl.start(&|| ready(root)).map_err(error)?;
        }
        Command::Stop => ctl.stop(&|| ready(root)).map_err(error)?,
        Command::Uninstall => ctl.uninstall(&|| ready(root)).map_err(error)?,
        Command::Status => (),
    }
    if matches!(command, Command::Status) {
        return status(root);
    }
    ctl.status(&|| ready(root)).map_err(error)
}

fn serve_arguments(root: &Path, logging: &hive_log::Config) -> Result<Vec<String>> {
    logging.validate()?;
    if !logging.file_bytes.is_multiple_of(1024 * 1024) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Service log size must be a whole MiB",
        )
        .into());
    }
    Ok(vec![
        "--root".into(),
        hive_setup_support::service::path_text(&files::absolute(root)?)?,
        "serve".into(),
        "--log-level".into(),
        logging.level.to_string().to_ascii_lowercase(),
        "--log-file-mib".into(),
        (logging.file_bytes / (1024 * 1024)).to_string(),
        "--log-files".into(),
        logging.files.to_string(),
    ])
}
pub fn start_for_setup(
    root: &Path,
    preserve: bool,
) -> Result<hive_setup_support::service::StartDecision> {
    validate_environment(root, &required_environment(root)?)?;
    let layout = Layout::discover()?;
    let scope = scope(root)?;
    Controller {
        layout: &layout,
        scope: &scope,
        runner: &SystemRunner(std::process::Command::spawn),
    }
    .start_for_setup(&|| ready(root), preserve)
    .map_err(error)
}
pub fn paused<T>(root: &Path, apply: impl FnOnce() -> Result<T>) -> Result<T> {
    let layout = Layout::discover()?;
    let scope = scope(root)?;
    let runner = SystemRunner(std::process::Command::spawn);
    Controller {
        layout: &layout,
        scope: &scope,
        runner: &runner,
    }
    .paused(&|| ready(root), || apply().map_err(std::io::Error::other))
    .map_err(error)
}
pub fn owned(root: &Path) -> Result<bool> {
    Ok(hive_setup_support::service::load(&Layout::discover()?, &scope(root)?)?.is_some())
}
pub fn required_environment(root: &Path) -> Result<Vec<String>> {
    if !root.try_exists()? {
        return Ok(vec![]);
    }
    // The accepted journal is what recovery will activate before serving.
    let settings = match provision::pending_settings(root)? {
        Some(pending) => Some(pending),
        None => provision::settings(root)?,
    };
    Ok(match settings.and_then(|s| s.embeddings) {
        Some(crate::embedding::Config::Http(config)) => config.api_key_env.into_iter().collect(),
        _ => vec![],
    })
}
pub fn validate_environment(root: &Path, names: &[String]) -> Result<()> {
    if let Some(spec) = hive_setup_support::service::load(&Layout::discover()?, &scope(root)?)? {
        let missing =
            hive_setup_support::environment::missing(spec.environment_file.as_deref(), names)?;
        if !missing.is_empty() {
            return Err(Error::service_model_environment_missing(&missing));
        }
    }
    Ok(())
}
pub fn setup(
    root: &Path,
    socket: Option<&Path>,
    input: crate::setup::Prepared,
) -> Result<crate::setup::Outcome> {
    setup::execute(root, socket, input)
}

const HEALTH_TIMEOUT: std::time::Duration = Duration::from_secs(2);
