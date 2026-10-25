//! Migration admission is outermost; normal operations clone its shared owner
//! before taking daemon, credential, index, or evidence locks.
use super::State;
use crate::error::{Error, Result};
pub(crate) use hive_setup_support::admission::Shared;
mod migration;
pub use migration::MigrationOwnership;

impl State {
    /// Retain admission after validating the format once for this operation.
    /// Runtime admission already validates the held proof; standalone callers
    /// must additionally inspect the current index and database identity.
    pub(super) fn format_admission(&self) -> Result<Shared> {
        let owner = self.admission()?;
        if self.runtime.is_none() {
            self.check_format_owned(&owner)?;
        }
        Ok(owner)
    }

    pub(crate) fn admission(&self) -> Result<Shared> {
        if let Some(runtime) = &self.runtime {
            runtime.check(self)?;
            return Ok(runtime.owner.clone());
        }
        let owner = Shared::acquire(&self.root, "hive-state").map_err(Error::state_io_from)?;
        super::format::check(&self.root)?;
        owner
            .admit_writes(
                &self.root,
                &super::journal::Boundary(&self.root.join(hive_setup_support::migration::PENDING)),
            )
            .map_err(Error::state_io_from)?;
        Ok(owner)
    }
}

mod runtime;
pub(super) use runtime::Runtime;

pub(crate) struct Initialization {
    // Field order releases inner publication ownership before root admission.
    pub(super) _publication: hive_setup_support::locks::Guard,
    pub(super) _admission: Shared,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{Method, Profile, Provider},
        state::OperationKind,
    };
    use hive_setup_support::admission::Exclusive;

    pub(super) fn fixture() -> (tempfile::TempDir, State, crate::state::Binding) {
        let temp = tempfile::tempdir().unwrap();
        let state = State::new(temp.path().join("state")).unwrap();
        let binding = state
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
        (temp, state, binding)
    }

    #[test]
    fn exclusive_admission_fences_every_standalone_writer_before_its_first_mutation() {
        let (_temp, state, binding) = fixture();
        let owner = Exclusive::existing(&state.root, "hive-state").unwrap();
        let before = std::fs::read(state.root.join("index.json")).unwrap();
        assert!(state.register("another", &binding.profile).is_err());
        assert!(state.begin_repair("main", &binding.profile).is_err());
        assert!(state.metadata_probe().is_err());
        assert!(state.record_account(&binding, None).is_err());
        assert!(state.recovery_report(&binding).is_err());
        assert!(state.runtime_lease(&binding).is_err());
        assert!(crate::runtime::maintenance_lock(&state.root).is_err());
        for kind in [
            OperationKind::Login,
            OperationKind::Status,
            OperationKind::Models,
            OperationKind::Recover,
        ] {
            assert!(state.operation(&binding, kind).is_err());
        }
        assert_eq!(
            std::fs::read(state.root.join("index.json")).unwrap(),
            before
        );
        assert!(!binding.directory.join("recovery-receipts").exists());
        assert!(!state.root.join("native-probes").exists());
        assert!(!state.root.join("runtime").exists());
        drop(owner);
        state.register("another", &binding.profile).unwrap();
    }

    #[test]
    fn normal_writers_retry_the_removed_migration_intent_barrier() {
        let (_temp, state, binding) = fixture();
        super::super::directory(&state.root.join("migrations")).unwrap();
        let before = std::fs::read(state.root.join("index.json")).unwrap();
        for _ in 0..2 {
            let result = super::super::sync_tests::with_hook(
                |path, step| {
                    if path
                        .file_name()
                        .is_some_and(|name| name == hive_setup_support::migration::PENDING)
                        && step == atomic_fs::Step::BeforeDirectorySync
                    {
                        return Err(std::io::Error::from_raw_os_error(libc::EIO));
                    }
                    Ok(())
                },
                || state.register("after-upgrade", &binding.profile),
            );
            assert!(result.is_err());
            assert_eq!(
                std::fs::read(state.root.join("index.json")).unwrap(),
                before
            );
        }
        state.register("after-upgrade", &binding.profile).unwrap();
    }

    #[test]
    fn detached_receipt_and_repair_hold_root_ownership() {
        let (_temp, state, binding) = fixture();
        let mut operation = state.operation(&binding, OperationKind::Status).unwrap();
        let child = operation.prepare_child(false, false).unwrap();
        drop(operation);
        assert!(Exclusive::existing(&state.root, "hive-state").is_err());
        child.not_started().unwrap();
        drop(child);
        drop(Exclusive::existing(&state.root, "hive-state").unwrap());
        let repair = state.begin_repair("main", &binding.profile).unwrap();
        assert!(Exclusive::existing(&state.root, "hive-state").is_err());
        state.finish_repair(&repair, false, None).unwrap();
        drop(repair);
        drop(Exclusive::existing(&state.root, "hive-state").unwrap());
    }

    #[test]
    fn migration_requires_original_native_lock_after_rust_owner_has_gone() {
        let (_temp, state, binding) = fixture();
        let operation = state.operation(&binding, OperationKind::Status).unwrap();
        // dup shares the exact description inherited by a launched native child.
        let inherited = operation.fd().try_clone_to_owned().unwrap();
        drop(operation);
        // The root gate is free, but the native namespace is still occupied.
        drop(Exclusive::existing(&state.root, "hive-state").unwrap());
        assert!(state.migration_ownership().is_err());
        rustix::fs::flock(&inherited, rustix::fs::FlockOperation::Unlock).unwrap();
        drop(inherited);
        let migration = state.migration_ownership().unwrap();
        migration.check().unwrap();
        assert!(state.migration_ownership().is_err());
        assert!(state.operation(&binding, OperationKind::Login).is_err());
        let path = binding.directory.join("auth.lock");
        std::fs::rename(&path, path.with_extension("retained")).unwrap();
        assert!(migration.check().is_err());
        drop(migration);
        assert!(state.migration_ownership().is_err());
        assert!(!path.exists());
    }
}
