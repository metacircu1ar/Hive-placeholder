//! Short-lived native metadata namespaces; no credential generation is allocated.
use super::{Error, Lock, LockIdentity, ProcessIdentity, Result, State, Uuid, exists, ledger};
use crate::state::{self, check_path, corrupt, read_json, write_json};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};
mod removal;
mod wrapper;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Launch {
    Unallocated,
    Receipt(Uuid),
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    NativeMetadataProbe,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    kind: Kind,
    id: Uuid,
    owner: ProcessIdentity,
    boot: String,
    lock: LockIdentity,
    launch: Launch,
}

pub(super) fn publish(dir: &Path, id: Uuid, lock: &Lock) -> Result<()> {
    write_json(
        &dir.join("identity.json"),
        &Manifest {
            kind: Kind::NativeMetadataProbe,
            id,
            owner: ProcessIdentity::current()?,
            boot: state::boot_identity()?,
            lock: LockIdentity::from_fd(lock.fd())?,
            launch: Launch::Unallocated,
        },
    )
}
pub(super) fn reserve(dir: &Path, id: Uuid) -> Result<()> {
    let path = dir.join("identity.json");
    let mut manifest: Manifest = read_json(&path)?;
    if !matches!(manifest.launch, Launch::Unallocated) {
        return Err(corrupt());
    }
    manifest.launch = Launch::Receipt(id);
    write_json(&path, &manifest)
}
pub(super) fn publication_lock(state: &State) -> Result<Lock> {
    let path = state.root.join("native-probes.lock");
    let lock = Lock::index(&path)?;
    // An empty-wrapper remover may unlink a held lock. Never admit an old-inode waiter.
    if LockIdentity::from_fd(lock.fd())? != LockIdentity::at(&path)? {
        return Err(state::unsafe_state());
    }
    Ok(lock)
}
pub(super) fn allocate(dir: &Path) -> Result<()> {
    removal::Paths::new(dir)?.allocate()
}
impl State {
    /// Best effort: unknown or live evidence stays, and cleanup cannot deny login.
    pub(crate) fn sweep_metadata_probes(&self) {
        if let Err(error) = self.sweep_metadata_probes_checked() {
            cleanup_failed("sweep_probes", &error);
        }
    }
    fn sweep_metadata_probes_checked(&self) -> Result<()> {
        let _admission = self.admission()?;
        self.check_format()?;
        if !exists(&self.root)? {
            return Ok(());
        }
        let initialization = self.initialization_lock()?;
        // Cleanup may have removed the empty wrapper since the preflight. Never
        // recreate a lock in an absent or not-yet-initialized root.
        if !crate::state::format::check(&self.root)? {
            return Ok(());
        }
        let publication = publication_lock(self)?;
        drop(initialization);
        let base = self.root.join("native-probes");
        if !exists(&base)? {
            return Ok(());
        }
        check_path(&base, true)?;
        state::publication::cleanup::sweep(&base, || {
            state::publication::cleanup::ownership(
                &publication,
                &self.root.join("native-probes.lock"),
            )
        })?;
        for entry in fs::read_dir(&base).map_err(Error::state_io_from)? {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    cleanup_failed("read_probe_entry", &error);
                    continue;
                }
            };
            if let Some(id) = removal::entry_id(&entry.file_name())
                && let Err(error) = reclaim(&base.join(id.to_string()), None)
            {
                cleanup_failed("reclaim_probe", &error);
            }
        }
        // A previous permit unlink can be visible before its parent sync failed.
        state::journal::Boundary(&base).sync_directory()
    }
}
/// Only the consumed Operation may provide this local completion token. It has
/// dropped its receipts; surviving clones or inherited descriptors keep the lock busy.
pub(super) fn finish(state: &State, dir: &Path, id: Uuid) {
    if let Err(error) = finish_probe(state, dir, id) {
        cleanup_failed("finish_probe", &error);
    }
}
fn cleanup_failed(phase: &'static str, error: &(dyn std::error::Error + 'static)) {
    static FAILURES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if let Some(count) = crate::observability::sample(&FAILURES) {
        tracing::warn!(name: "recovery.probe_cleanup_failed",
             phase = phase,
             error = error,
             count = count,
             decision = "retain_evidence");
    }
}
fn finish_probe(state: &State, dir: &Path, id: Uuid) -> Result<()> {
    let _admission = state.admission()?;
    let initialization = state.initialization_lock()?;
    let _publication = publication_lock(state)?;
    drop(initialization);
    reclaim(dir, Some(id))
}
fn reclaim(dir: &Path, finished: Option<Uuid>) -> Result<()> {
    let paths = removal::Paths::new(dir)?;
    if paths.permitted()? {
        return paths.remove();
    }
    if !exists(dir)? {
        return state::journal::Boundary(dir).sync_parent();
    }
    let Some(lock) = authorize(dir, finished)? else {
        return Ok(());
    };
    paths.permit(&lock)?;
    paths.remove()
}
fn authorize(dir: &Path, finished: Option<Uuid>) -> Result<Option<Lock>> {
    check_path(dir, true)?;
    let manifest: Manifest = read_json(&dir.join("identity.json"))?;
    if dir.file_name().and_then(|s| s.to_str()) != Some(&manifest.id.to_string()) {
        return Err(corrupt());
    }
    let local = finished == Some(manifest.id) && manifest.owner == ProcessIdentity::current()?;
    if !local && !manifest.owner.absent()? {
        return Ok(None);
    }
    let lock = Lock::acquire_existing(&dir.join("auth.lock"))?;
    if LockIdentity::from_fd(lock.fd())? != manifest.lock {
        return Err(corrupt());
    }
    super::layout::check(dir, super::layout::Namespace::Probe)?;
    let active = ledger::records_in(dir, "executions")?;
    let archive = ledger::records_in(dir, "execution-archive")?;
    let preparation = archive.is_empty()
        && matches!(active.as_slice(), [record] if matching(&manifest, record) && pristine(record));
    match (&manifest.launch, active.as_slice(), archive.as_slice()) {
        (Launch::Unallocated, [], []) => {}
        (Launch::Unallocated, [_], []) if preparation => {}
        (Launch::Receipt(id), [record], []) | (Launch::Receipt(id), [], [record])
            if record.id == *id
                && matching(&manifest, record)
                && (record.confirmed
                    || (local && preparation)
                    || ledger::ownership_gone(record)?) => {}
        _ => return Err(corrupt()),
    }
    Ok(Some(lock))
}

pub(in crate::state) fn migration_probes(state: &State) -> Result<Vec<(std::path::PathBuf, Lock)>> {
    let base = state.root.join("native-probes");
    if !exists(&base)? {
        return Ok(Vec::new());
    }
    check_path(&base, true)?;
    let mut locks = Vec::new();
    for entry in fs::read_dir(&base).map_err(Error::state_io_from)? {
        let entry = entry.map_err(Error::state_io_from)?;
        let dir = entry.path();
        // Do not skip partial retirement/preparation evidence during an upgrade.
        check_path(&dir, true)?;
        let manifest: Manifest = read_json(&dir.join("identity.json"))?;
        if entry.file_name().to_str() != Some(&manifest.id.to_string())
            || (crate::state::same_boot(&manifest.boot)? && !manifest.owner.absent()?)
        {
            return Err(super::recovery_required());
        }
        super::layout::check(&dir, super::layout::Namespace::Probe)?;
        let path = dir.join("auth.lock");
        let lock = Lock::acquire_existing(&path)?;
        if LockIdentity::from_fd(lock.fd())? != manifest.lock {
            return Err(corrupt());
        }
        let active = ledger::records_owned(&dir, "executions")?;
        let archive = ledger::records_owned(&dir, "execution-archive")?;
        match (&manifest.launch, active.as_slice(), archive.as_slice()) {
            (Launch::Unallocated, [], []) => {}
            (Launch::Unallocated, [record], [])
                if matching(&manifest, record) && pristine(record) => {}
            (Launch::Receipt(id), [record], []) | (Launch::Receipt(id), [], [record])
                if record.id == *id
                    && matching(&manifest, record)
                    && ledger::migration_safe(record)? => {}
            _ => return Err(super::recovery_required()),
        }
        locks.push((path, lock));
    }
    Ok(locks)
}

fn matching(manifest: &Manifest, record: &ledger::Record) -> bool {
    record.generation == manifest.id
        && record.run == manifest.id
        && record.owner == manifest.owner
        && record.boot == manifest.boot
        && record.lock == manifest.lock
        && record.class == ledger::Class::Probe
}

fn pristine(record: &ledger::Record) -> bool {
    // This exact state precedes both the launch handle and native authorization.
    record.phase == ledger::ExecutionPhase::Intent
        && record.revision == 0
        && record.never_authorized
        && record.process.is_none()
        && record.conversation.is_none()
        && record.account_identity.is_none()
        && !record.quiescent
        && !record.confirmed
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[test]
fn probe_manifest_requires_exact_fields_and_known_launch_state() {
    crate::state::assert_strict_record::<Manifest>(
        &serde_json::json!({
            "kind": "native_metadata_probe", "id": Uuid::nil(),
            "owner": {"pid": 1, "birth": "macos:1:0"}, "boot": Uuid::nil().to_string(),
            "lock": {"dev": 1, "ino": 1}, "launch": "unallocated",
        }),
        &["kind", "launch"],
    );
}
