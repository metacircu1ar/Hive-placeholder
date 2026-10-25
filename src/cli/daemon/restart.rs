//! Restart never signals an unverified PID or replays an uncertain shutdown.
use super::{CommandOutcome, Error, PathBuf, Request, Result, State, runtime};
use std::time::Instant;

pub(super) fn run(
    config: PathBuf,
    root: PathBuf,
    binaries: runtime::Binaries,
    request_id: &str,
) -> Result<CommandOutcome> {
    let _configuration = runtime::daemon_configuration(&config)?;
    let admission = State::new(root.clone())?.admission()?;
    let deadline = Instant::now() + super::DAEMON_START_TIMEOUT;
    let rt = crate::calls::executor()?;
    if let Some(value) =
        rt.block_on(runtime::client::Client::new(&root).startup(deadline.into()))?
    {
        if super::api::DaemonReply::new(&value)?.has_error() {
            return Ok(CommandOutcome::failed(value));
        }
        let stopped = super::remote_response(&root, request_id, Request::Shutdown)?;
        if !stopped.success {
            return Ok(stopped);
        }
    }
    // A daemon already shutting down may have removed its socket before this
    // request. Check its original ownership even when the probe found no listener.
    // Stopping gets its own budget: provider shutdown graces outlast the probe's.
    cleanup(&root, Instant::now() + super::super::DAEMON_STOP_TIMEOUT)?;
    let result = super::start_daemon(config, root, binaries);
    drop(admission);
    result
}

pub(in crate::cli) fn cleanup(root: &std::path::Path, deadline: Instant) -> Result<()> {
    if !root
        .join("runtime")
        .try_exists()
        .map_err(Error::state_io_from)?
    {
        return Ok(());
    }
    loop {
        match runtime::purge_lock(root) {
            Ok(owner) => {
                drop(owner);
                return Ok(());
            }
            Err(error) if matches!(error.code(), "daemon_busy" | "recovery_required") => {
                if Instant::now() >= deadline {
                    return Err(Error::daemon_start_timeout(
                        "Daemon cleanup is unconfirmed; restart did not launch another process.",
                    )
                    .with_source(error));
                }
                std::thread::sleep(super::DAEMON_START_POLL);
            }
            Err(error) => return Err(error),
        }
    }
}
#[cfg(test)]
#[test]
fn missing_socket_does_not_admit_restart_while_original_owner_is_held() {
    let temp = crate::runtime::test_directory();
    let state = State::new(temp.path().into()).unwrap();
    let initialization = state.initialize().unwrap();
    drop(initialization);
    let owner = runtime::purge_lock(temp.path()).unwrap();
    assert!(!runtime::socket_path(temp.path()).exists());
    assert_eq!(
        cleanup(temp.path(), Instant::now()).unwrap_err().code(),
        "daemon_start_timeout"
    );
    drop(owner);
    cleanup(temp.path(), Instant::now()).unwrap();
}
