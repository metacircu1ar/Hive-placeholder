//! Standalone server diagnostics span binding, service work and runtime teardown.
use comb::{Error, Result, service::Prepared};
use std::time::Duration;

const FINISH_BUDGET: Duration = Duration::from_secs(1);
pub(super) struct ProcessLog(hive_log::Guard);
impl ProcessLog {
    pub fn start(prepared: &Prepared, config: hive_log::Config) -> Result<Self> {
        let (backend, guard) = prepared.diagnostics(config)?;
        if let Err(cause) = backend.clone().install_global() {
            guard.finish(FINISH_BUDGET);
            return Err(Error::from(std::io::Error::other(cause)));
        }
        backend.install_panic_hook();
        tracing::info!(name: "comb.starting", pid = std::process::id());
        Ok(Self(guard))
    }
    pub fn finish<T>(self, result: Result<T>) -> Result<T> {
        if let Err(error) = &result {
            tracing::error!(name: "comb.failed",
                 target: "hive.fatal",
                 code = error.code.as_str(),
                 error = error as &(dyn std::error::Error + 'static),
                 decision = "shutdown");
        }
        tracing::info!(name: "comb.stopped", success = result.is_ok());
        let finished = self.0.finish(FINISH_BUDGET);
        if finished.state != hive_log::Shutdown::Drained && result.is_ok() {
            return Err(Error::from(std::io::Error::other(
                "Diagnostic log shutdown incomplete.",
            )));
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use comb::{
        Comb, ErrorCode,
        service::{Options, Server},
    };
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn startup_and_global_install_failures_release_ownership_and_log_outcome() {
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        drop(Comb::init(directory.path()).unwrap());
        let prepared = Server::prepare(directory.path(), Options::default()).unwrap();
        let logging = ProcessLog::start(&prepared, hive_log::Config::default()).unwrap();
        let blocker =
            std::os::unix::net::UnixListener::bind(directory.path().join(".comb/service.sock"))
                .unwrap();
        std::fs::set_permissions(
            directory.path().join(".comb/service.sock"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        let error = logging.finish(prepared.bind()).err().unwrap();
        assert_eq!(error.code, ErrorCode::CombBusy);
        let log = std::fs::read_to_string(directory.path().join(".comb/logs/comb.jsonl")).unwrap();
        assert!(log.contains("comb.failed"));
        assert!(log.contains("comb.stopped"));
        assert!(log.contains("comb_busy"));
        assert!(!log.contains("comb.ready"));
        drop(blocker);
        std::fs::remove_file(directory.path().join(".comb/service.sock")).unwrap();
        drop(Comb::open(directory.path()).unwrap());
        let prepared = Server::prepare(directory.path(), Options::default()).unwrap();
        assert_eq!(
            ProcessLog::start(&prepared, hive_log::Config::default())
                .err()
                .unwrap()
                .code,
            ErrorCode::IoError
        );
        drop(prepared);
        // A failed global registration drained its unused backend and releases ownership.
        drop(Comb::open(directory.path()).unwrap());
    }
}
