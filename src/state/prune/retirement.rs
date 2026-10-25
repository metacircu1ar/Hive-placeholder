//! Durable intent separates removal from empty-directory reconstruction.
use super::{Binding, GenerationState, LockIdentity, Operation, Outcome, PruneReceipt, journal};
use crate::{
    error::{Error, Result},
    publication,
    state::{self, corrupt, recovery::exists},
};
use atomic_fs::{Barriers, DirectoryIdentity, Step};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::Path,
};
use uuid::Uuid;

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct TreeIdentity {
    device: u64,
    inode: u64,
}
impl TreeIdentity {
    fn at(path: &Path) -> Result<Self> {
        state::check_path(path, true)?;
        let metadata = fs::symlink_metadata(path).map_err(Error::state_io_from)?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    generation: Uuid,
    profile: String,
    credential_lock: LockIdentity,
    provider: TreeIdentity,
    work: TreeIdentity,
}
impl Intent {
    fn load(binding: &Binding, name: &str, lock: &LockIdentity) -> Result<Option<Self>> {
        let path = binding.directory.join("prune-intent.json");
        if !exists(&path)? {
            return Ok(None);
        }
        let intent: Self = state::read_json(&path)?;
        if intent.generation != binding.id
            || intent.profile != name
            || intent.credential_lock != *lock
        {
            return Err(corrupt());
        }
        Ok(Some(intent))
    }
    fn capture(binding: &Binding, name: &str, lock: &LockIdentity) -> Result<Self> {
        pre_intent(&binding.directory)?;
        Ok(Self {
            generation: binding.id,
            profile: name.into(),
            credential_lock: lock.clone(),
            provider: TreeIdentity::at(&binding.directory.join("provider"))?,
            work: TreeIdentity::at(&binding.directory.join("work"))?,
        })
    }
    fn trees(&self) -> [(&str, TreeIdentity); 2] {
        [("provider", self.provider), ("work", self.work)]
    }
    fn validate_trees(&self, dir: &Path) -> Result<()> {
        for (name, identity) in self.trees() {
            let original = dir.join(name);
            let aside = dir.join(format!("{name}.pruning"));
            match (exists(&original)?, exists(&aside)?) {
                (true, false) if TreeIdentity::at(&original)? == identity => {}
                (false, true) if TreeIdentity::at(&aside)? == identity => {}
                (false, false) => {}
                _ => return Err(corrupt()),
            }
        }
        Ok(())
    }
    fn remove(&self, dir: &Path) -> Result<()> {
        self.validate_trees(dir)?;
        for (name, identity) in self.trees() {
            let original = dir.join(name);
            atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
                .with_barriers(&journal::Boundary(&original))
                .remove_named_dir(
                    &original,
                    dir.join(format!("{name}.pruning")),
                    DirectoryIdentity::new(identity.device, identity.inode),
                )
                .map_err(publication::error)?;
        }
        Ok(())
    }
}

fn pre_intent(dir: &Path) -> Result<()> {
    if exists(&dir.join("prune-receipt.json"))? {
        return Err(corrupt());
    }
    for name in ["provider", "work"] {
        state::check_path(&dir.join(name), true)?;
        if exists(&dir.join(format!("{name}.pruning")))? {
            return Err(corrupt());
        }
    }
    Ok(())
}
fn reconstruction(dir: &Path, terminal: bool) -> Result<()> {
    for name in ["provider", "work"] {
        if exists(&dir.join(format!("{name}.pruning")))? {
            return Err(corrupt());
        }
        let path = dir.join(name);
        if exists(&path)? {
            state::check_path(&path, true)?;
            if let Some(entry) = fs::read_dir(&path).map_err(Error::state_io_from)?.next() {
                entry.map_err(Error::state_io_from)?;
                return Err(corrupt());
            }
        } else if terminal {
            return Err(corrupt());
        }
    }
    Ok(())
}
pub(super) fn validate(
    binding: &Binding,
    name: &str,
    lock: &LockIdentity,
    phase: GenerationState,
) -> Result<()> {
    let dir = &binding.directory;
    let intent = Intent::load(binding, name, lock)?;
    let receipt = PruneReceipt::load(&dir.join("prune-receipt.json"), name, binding.id)?;
    match (phase, intent, receipt) {
        (GenerationState::Retired | GenerationState::Pruning, None, None) => pre_intent(dir),
        (GenerationState::Pruning, Some(intent), None) => intent.validate_trees(dir),
        (GenerationState::Pruning | GenerationState::Pruned, Some(_), Some(_)) => {
            reconstruction(dir, phase == GenerationState::Pruned)
        }
        _ => Err(corrupt()),
    }
}

pub(super) fn finish(
    binding: &Binding,
    name: &str,
    lock: &LockIdentity,
    terminal: bool,
    native: impl FnOnce() -> Result<()>,
) -> Result<PruneReceipt> {
    let dir = &binding.directory;
    let intent_path = dir.join("prune-intent.json");
    let intent = if let Some(intent) = Intent::load(binding, name, lock)? {
        intent
    } else {
        if terminal {
            return Err(corrupt());
        }
        let intent = Intent::capture(binding, name, lock)?;
        journal::publish(&intent_path, &intent)?;
        intent
    };
    journal::sync_record(&intent_path)?;
    let receipt_path = dir.join("prune-receipt.json");
    let receipt = if let Some(receipt) = PruneReceipt::load(&receipt_path, name, binding.id)? {
        reconstruction(dir, terminal)?;
        journal::sync_record(&receipt_path)?;
        receipt
    } else {
        if terminal {
            return Err(corrupt());
        }
        intent.validate_trees(dir)?;
        native()?;
        intent.remove(dir)?;
        let receipt = PruneReceipt {
            account_sessions_revoked: false,
            checked_at: state::now(),
            generation: binding.id,
            local_credentials_removed: true,
            operation: Operation::Prune,
            outcome: Outcome::Pruned,
            profile: name.into(),
        };
        journal::publish(&receipt_path, &receipt)?;
        receipt
    };
    if !terminal {
        reconstruct(dir)?;
    }
    reconstruction(dir, true)?;
    Ok(receipt)
}

fn reconstruct(dir: &Path) -> Result<()> {
    reconstruction(dir, false)?;
    for name in ["provider", "work"] {
        let path = dir.join(name);
        let boundary = journal::Boundary(&path);
        boundary
            .observe(Step::BeforePublish)
            .map_err(Error::state_io_from)?;
        if !exists(&path)? {
            fs::DirBuilder::new()
                .mode(crate::os::PRIVATE_DIRECTORY_MODE)
                .create(&path)
                .map_err(Error::state_io_from)?;
        }
        state::check_path(&path, true)?;
        boundary
            .observe(Step::AfterPublish)
            .map_err(Error::state_io_from)?;
        boundary.sync_parent()?;
    }
    Ok(())
}

#[cfg(test)]
#[test]
fn prune_intent_requires_exact_identity_fields() {
    state::assert_strict_record::<Intent>(
        &serde_json::json!({
            "generation": Uuid::nil(), "profile": "main",
            "credential_lock": {"dev": 1, "ino": 2},
            "provider": {"device": 1, "inode": 3}, "work": {"device": 1, "inode": 4},
        }),
        &[],
    );
    state::assert_strict_record::<TreeIdentity>(&serde_json::json!({"device": 1, "inode": 2}), &[]);
}
