//! Compare the running daemon's startup snapshot, never the on-disk file alone.
mod publication;
use super::{Result, invalid, io, options::Values, publication::Snapshot};
use crate::{
    config::{Config, ConfigurationIdentity},
    runtime::{self, api::Request},
};
use hive_setup_support::{
    runner::SystemRunner,
    service::{self, Controller, Layout},
};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
#[derive(Deserialize, Serialize)]
pub(in crate::cli) struct Observation {
    pub product_version: String,
    pub binary_sha256: String,
    pub commit: Option<String>,
    pub protocols: hive_input::compatibility::ProtocolRange,
    pub agent_catalog: u32,
    pub state_format: u32,
    pub configuration: Option<ConfigurationIdentity>,
    pub resident_actors: usize,
    pub runtime_resources: Resources,
}
#[derive(Deserialize, Serialize)]
pub(in crate::cli) struct Resources {
    pub active_turns: usize,
}
pub(in crate::cli) fn observe(root: &Path) -> Result<Option<Observation>> {
    let result = crate::calls::executor()?.block_on(
        runtime::client::Client::new(root).observe::<Observation>(
            "setup",
            &Request::Ping,
            tokio::time::Instant::now() + OBSERVATION_TIMEOUT,
        ),
    );
    match result {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.code() == crate::error::ErrorCode::DaemonUnavailable => Ok(None),
        Err(e) => Err(invalid(format!("Cannot establish daemon state: {e:?}"))),
    }
}
pub(super) struct Publication {
    pub backup: Option<PathBuf>,
    pub restarted: bool,
}
pub(super) fn publish(
    config: &Path,
    root: &Path,
    snapshot: &Snapshot,
    text: &str,
    options: &Values,
) -> Result<Publication> {
    let loaded = observe(root)?;
    if let Some(loaded) = &loaded {
        eprintln!(
            "Daemon: {} loaded agents, {} active turns.",
            loaded.resident_actors, loaded.runtime_resources.active_turns
        );
    }
    let same = loaded
        .as_ref()
        .is_some_and(|l| matches_configuration(config, l));
    let layout = Layout::discover().map_err(io)?;
    let scope = super::super::services::scope(config, root)?;
    let owned = service::load(&layout, &scope).map_err(io)?.is_some();
    let changed = text != snapshot.text || !same;
    if changed && owned && options.restart_daemon == Some(true) {
        let label = scope.label().map_err(io)?;
        let controller = Controller {
            layout: &layout,
            scope: &scope,
            runner: &SystemRunner(crate::os::spawn),
        };
        let backup = publication::paused(
            &controller,
            &|| super::super::services::health(root, Some(&label)),
            snapshot,
            text,
        )?;
        Ok(Publication {
            backup,
            restarted: loaded.is_some(),
        })
    } else {
        Ok(Publication {
            backup: snapshot.publish(text)?,
            restarted: false,
        })
    }
}

pub(super) fn matches_configuration(config: &Path, loaded: &Observation) -> bool {
    // Setup can repair a missing/broken configuration. A failed read never
    // proves that a daemon with no configuration identity is already current.
    crate::observability::optional(Config::load(config), "setup_loaded_configuration")
        .is_some_and(|current| current.loaded.is_some() && current.loaded == loaded.configuration)
}
pub(super) fn services(
    cli: &super::super::Cli,
    config: &Path,
    root: &Path,
    options: &Values,
) -> Result<ServiceDecision> {
    if options.no_services == Some(true) {
        if observe(root)?.is_some() {
            return Ok(ServiceDecision::manual(vec![]));
        }
        return Ok(ServiceDecision::manual(vec![format!(
            "{} daemon run",
            super::super::doctor::command(cli, config, root, "daemon run")
        )]));
    }
    let mut state = super::super::services::status(config, root)?;
    if !state.manager_available {
        return Ok(ServiceDecision::manual(vec![state.manual_start]));
    }
    let preserve = options.preserve_stopped_services == Some(true) && state.managed;
    let mut changed = false;
    if !state.managed {
        if state.ready {
            return Ok(ServiceDecision::manual(vec![
                "Existing manual daemon retained; stop it manually before `hivectl service \
                    install`."
                    .into(),
            ]));
        }
        super::super::services::run_new(
            cli,
            config,
            root,
            &service::Command::Install {
                environment_file: options.service_environment.clone(),
                program: None,
                service_path: options.service_path.clone(),
            },
        )?;
        state = super::super::services::status(config, root)?;
        changed = true;
    }
    if !state.ready {
        let decision = start_for_setup(config, root, preserve)?;
        if decision == service::StartDecision::PreservedStopped {
            return Ok(ServiceDecision {
                changed,
                manual: true,
                pending: vec![format!(
                    "Stopped service retained; start with {} service start",
                    super::super::doctor::command(cli, config, root, "service start")
                )],
            });
        }
        changed |= decision == service::StartDecision::Started;
    }
    Ok(ServiceDecision {
        changed,
        pending: vec![],
        manual: false,
    })
}

pub(super) struct ServiceDecision {
    pub changed: bool,
    pub pending: Vec<String>,
    pub manual: bool,
}
impl ServiceDecision {
    fn manual(pending: Vec<String>) -> Self {
        Self {
            changed: false,
            pending,
            manual: true,
        }
    }
}

pub(super) fn preflight(config: &Path, root: &Path, options: &Values) -> Result<()> {
    let layout = Layout::discover().map_err(io)?;
    let scope = super::super::services::scope(config, root)?;
    let installed = service::load(&layout, &scope).map_err(io)?;
    let requested = options
        .service_environment
        .as_deref()
        .map(hive_setup_support::files::absolute)
        .transpose()
        .map_err(io)?;
    if let Some(spec) = &installed {
        if requested.is_some() && requested != spec.environment_file {
            return Err(invalid(
                "Existing Hive service uses another environment file; uninstall the service \
                    before changing it.",
            ));
        }
        if let Some(path) = &options.service_path {
            let path = hive_setup_support::environment::service_path(Some(path)).map_err(io)?;
            if spec.environment.get("PATH") != Some(&path) {
                return Err(invalid(
                    "Existing Hive service uses another PATH; uninstall the service before \
                        changing it.",
                ));
            }
        }
    }
    if let Some(path) = requested {
        hive_setup_support::environment::read(Some(&path)).map_err(io)?;
    }
    Ok(())
}

fn start_for_setup(config: &Path, root: &Path, preserve: bool) -> Result<service::StartDecision> {
    let layout = Layout::discover().map_err(io)?;
    let scope = super::super::services::scope(config, root)?;
    let label = scope.label().map_err(io)?;
    if let Some(spec) = service::load(&layout, &scope).map_err(io)? {
        super::super::services::validate_current(&spec)?;
    }
    let decision = Controller {
        layout: &layout,
        scope: &scope,
        runner: &SystemRunner(crate::os::spawn),
    }
    .start_for_setup(
        &|| super::super::services::health(root, Some(&label)),
        preserve,
    )
    .map_err(io)?;
    Ok(decision)
}

const OBSERVATION_TIMEOUT: std::time::Duration = Duration::from_secs(2);
