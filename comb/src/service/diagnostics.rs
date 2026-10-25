//! Opt-in diagnostics keep the service ownership through the final writer close.
use super::Prepared;
use crate::{Result, disk};

impl Prepared {
    /// Start an owned log backend. The executable decides how to install it;
    /// embedded servers use their host's tracing subscriber instead.
    pub fn diagnostics(
        &self,
        config: hive_log::Config,
    ) -> Result<(hive_log::Backend, hive_log::Guard)> {
        let directory = self.state.writer.lock()?.disk.admin.join("logs");
        disk::dir(&directory, true, &disk::System)?;
        Ok(hive_log::start_owned(
            &directory.join("comb.jsonl"),
            config,
            self.state.clone(),
        )?)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        Comb, ErrorCode,
        service::{Options, Server},
    };
    #[test]
    fn log_writer_retains_comb_until_explicit_drain() {
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        drop(Comb::init(directory.path()).unwrap());
        let prepared = Server::prepare(directory.path(), Options::default()).unwrap();
        let (backend, guard) = prepared.diagnostics(hive_log::Config::default()).unwrap();
        drop(prepared);
        assert_eq!(
            Comb::open(directory.path()).err().unwrap().code,
            ErrorCode::CombBusy
        );
        assert_eq!(
            guard.finish(std::time::Duration::from_secs(1)).state,
            hive_log::Shutdown::Drained
        );
        // A retained subscriber producer must not retain service ownership.
        let _reopened = Comb::open(directory.path()).unwrap();
        drop(backend);
    }
}
