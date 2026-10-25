use super::State;
use crate::{
    compatibility,
    error::{Error, Result},
};
use hive_setup_support::migration::{Backup, Engine, Receipt};
use std::path::Path;

#[derive(serde::Serialize)]
pub struct UpgradeReport {
    pub format: u32,
    pub dry_run: bool,
    pub status: &'static str,
    pub steps: Vec<(u32, u32)>,
    pub migrations: Vec<Receipt>,
    pub backups: Vec<Backup>,
}

impl State {
    /// No released predecessors exist yet. Current roots receive a complete
    /// exclusive preflight; development shapes are never assigned an adapter.
    pub fn upgrade(&self, dry_run: bool) -> Result<UpgradeReport> {
        let owner = self.migration_ownership()?;
        let check = || owner.check().map_err(std::io::Error::other);
        let root = self.root.canonicalize().map_err(Error::state_io_from)?;
        let engine =
            Engine::new(&root, &root, Path::new("format"), &check).map_err(Error::state_io_from)?;
        if engine.pending().map_err(Error::state_io_from)?.is_some() {
            return Err(Error::recovery_required(
                "A pending migration needs its registered released-format reader; retain \
                    this directory and its backups.",
            ));
        }
        crate::runtime::inspect_migration_database(&self.root.join("runtime/runtime.sqlite"))?;
        let migrations = engine.history().map_err(Error::state_io_from)?;
        let backups = engine.backups().map_err(Error::state_io_from)?;
        owner.check()?;
        Ok(UpgradeReport {
            format: compatibility::STATE.current,
            dry_run,
            status: "current",
            steps: Vec::new(),
            migrations,
            backups,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Method, Profile, Provider};

    #[test]
    fn inspection_is_read_only_and_never_adopts_missing_or_pending_state() {
        let temp = tempfile::tempdir().unwrap();
        let state = State::new(temp.path().join("state")).unwrap();
        assert!(state.upgrade(true).is_err());
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
        state
            .register(
                "main",
                &Profile {
                    provider: Provider::Codex,
                    method: Method::Subscription,
                    api_key_env: None,
                    api_key_file: None,
                },
            )
            .unwrap();
        let index = std::fs::read(state.root.join("index.json")).unwrap();
        for dry_run in [true, false] {
            let report = state.upgrade(dry_run).unwrap();
            assert!(report.steps.is_empty());
            assert!(report.backups.is_empty());
            assert_eq!(std::fs::read(state.root.join("index.json")).unwrap(), index);
            assert!(!state.root.join("backups").exists());
        }
        hive_setup_support::files::write(
            &state.root.join(hive_setup_support::migration::PENDING),
            b"retained malformed intent",
            false,
        )
        .unwrap();
        assert_eq!(
            state.check_format().unwrap_err().code(),
            "recovery_required"
        );
        assert!(state.upgrade(false).is_err());
        assert_eq!(std::fs::read(state.root.join("index.json")).unwrap(), index);
    }
}
