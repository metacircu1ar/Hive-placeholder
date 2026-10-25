//! Daemon control commands and bounded background startup.
pub(super) mod restart;
pub(super) mod selection;
mod startup_log;
use super::{
    CommandOutcome, DAEMON_START_POLL, DAEMON_START_TIMEOUT, DaemonCommand, Error, Instant,
    PathBuf, Request, Result, State, api, remote_response, runtime,
};

pub(super) fn daemon_command(
    command: &DaemonCommand,
    config_path: PathBuf,
    root: PathBuf,
    binaries: runtime::Binaries,
    request_id: &str,
) -> Result<CommandOutcome> {
    match command {
        DaemonCommand::Run => {
            runtime::serve(config_path, root, binaries)?;
            Ok(CommandOutcome::ok(api::value(api::StatusResponse {
                status: "stopped",
            })))
        }
        DaemonCommand::Start => start_daemon(config_path, root, binaries),
        DaemonCommand::Restart => restart::run(config_path, root, binaries, request_id),
        DaemonCommand::Stop => remote_response(&root, request_id, Request::Shutdown),
        DaemonCommand::Status => remote_response(&root, request_id, Request::Ping),
    }
}

// Keep ownership of the launch paths through startup and its bounded readiness wait.
pub(super) fn start_daemon(
    config: PathBuf,
    root: PathBuf,
    binaries: runtime::Binaries,
) -> Result<CommandOutcome> {
    use std::{
        os::unix::process::CommandExt,
        process::{Command, Stdio},
    };
    runtime::validate_logging()?;
    let expected = selection::Expected::load(&config, Some(&binaries), None)?;
    let state = State::new(root.clone())?;
    let _admission = state.admission()?;
    state.check_format()?;
    let deadline = Instant::now() + DAEMON_START_TIMEOUT;
    if let Some(outcome) = expected.observe(&root, deadline)? {
        return Ok(outcome);
    }
    let config = &expected.configuration.path;
    std::fs::create_dir_all(
        root.parent()
            .ok_or(Error::invalid_path("Invalid state root."))?,
    )
    .map_err(Error::state_io_from)?;
    let initialization = state.initialize()?;
    crate::state::directory(&root.join("runtime"))?;
    let log = startup_log::open(&root.join("runtime/daemon.log"))?;
    // The child takes this same initialization lock. Release after the first
    // owned records are published, before spawning and waiting for readiness.
    drop(initialization);
    let mut cmd = Command::new(crate::executable::path()?);
    cmd.arg("--config")
        .arg(config)
        .arg("--state-dir")
        .arg(&root)
        .arg("--codex-bin")
        .arg(binaries.codex)
        .arg("--claude-bin")
        .arg(binaries.claude)
        .args(["daemon", "run"])
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(Error::state_io_from)?)
        .stderr(log);
    // SAFETY: new_session performs only setsid and allocation-free errno conversion.
    unsafe {
        cmd.pre_exec(crate::os::new_session);
    }
    let mut child = crate::os::spawn(&mut cmd).map_err(Error::state_io_from)?;
    wait_for_daemon(&root, &mut child, &expected, deadline)
}

fn wait_for_daemon(
    root: &std::path::Path,
    child: &mut std::process::Child,
    expected: &selection::Expected,
    deadline: Instant,
) -> Result<CommandOutcome> {
    while Instant::now() < deadline {
        match expected.observe(root, deadline) {
            Ok(Some(outcome)) => return Ok(outcome),
            Ok(None) => (),
            Err(error) => {
                // The socket may have bound before startup failed. Prefer the
                // confirmed child exit, retaining the socket failure as its cause.
                return Err(match wait_for_exit(child, deadline) {
                    Ok(Some(status)) => exited(status).with_source(error),
                    Ok(None) => error,
                    Err(secondary) => {
                        tracing::warn!(name: "daemon.start_observation_failed",
                             primary = &error as &(dyn std::error::Error + 'static),
                             secondary = &secondary as &(dyn std::error::Error + 'static),
                             decision = "preserve_primary");
                        error
                    }
                });
            }
        }
        if let Some(status) = child.try_wait().map_err(Error::state_io_from)? {
            return Err(exited(status));
        }
        std::thread::sleep(DAEMON_START_POLL);
    }
    Err(Error::daemon_start_timeout(
        "Daemon has not become ready; inspect its log before retrying.",
    ))
}

fn exited(status: std::process::ExitStatus) -> Error {
    use std::os::unix::process::ExitStatusExt;
    tracing::error!(name: "daemon.start_child_exited",
         exit_code = status.code(),
         signal = status.signal(),
         decision = "inspect_logs");
    let classification = if let Some(code) = status.code() {
        format!("status {code}")
    } else if let Some(signal) = status.signal() {
        format!("signal {signal}")
    } else {
        "unknown status".into()
    };
    Error::DaemonStartFailed {
        message: format!(
            "Daemon exited with {classification}; inspect \
            runtime/logs/hive-daemon.jsonl and runtime/daemon.log in the state \
            directory."
        )
        .into(),
        source: None,
    }
}

// Runtime and logger shutdown can trail socket close. Wait only for our child,
// using the original deadline; never retry the ambiguous control exchange.
fn wait_for_exit(
    child: &mut std::process::Child,
    deadline: Instant,
) -> Result<Option<std::process::ExitStatus>> {
    loop {
        if let Some(status) = child.try_wait().map_err(Error::state_io_from)? {
            return Ok(Some(status));
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Ok(None);
        };
        std::thread::sleep(DAEMON_START_POLL.min(remaining));
    }
}
