//! Credential-only reconciliation never upgrades execution cleanup or quiescence.
use super::{
    Binding, Lock, LockIdentity, Result, State, Uuid,
    ledger::{self, Class, Record},
};
use crate::state::{self, check_path, corrupt, directory, read_json, write_json};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Resolution {
    generation: Uuid,
    run: Uuid,
    writer: Uuid,
    revision: u64,
    lock: LockIdentity,
    checked_at: u64,
}
fn path(dir: &Path, record: &Record) -> PathBuf {
    dir.join("credential-resolutions")
        .join(format!("{}.{}.json", record.id, record.revision))
}
pub(super) fn resolved(dir: &Path, record: &Record) -> Result<bool> {
    super::layout::audit_profile(dir)?;
    let base = dir.join("credential-resolutions");
    if super::exists(&base)? {
        check_path(&base, true)?;
    }
    let path = path(dir, record);
    if !super::exists(&path)? {
        return Ok(false);
    }
    let resolution: Resolution = read_json(&path)?;
    if resolution.generation != record.generation
        || resolution.run != record.run
        || resolution.writer != record.id
        || resolution.revision != record.revision
        || resolution.lock != record.lock
    {
        return Err(corrupt());
    }
    Ok(true)
}
pub(super) fn pending(state: &State, binding: &Binding, lock: &Lock) -> Result<Option<Record>> {
    super::layout::audit_profile(&binding.directory)?;
    let Some(attempt) = state.pending_credential_attempt(binding.id)? else {
        return Ok(None);
    };
    let dir = binding.directory.clone();
    let _evidence = Lock::index(&dir.join("evidence.lock"))?;
    let record = ledger::find_record(&dir, ["writers", "execution-archive"], attempt.writer)?
        .ok_or_else(corrupt)?;
    if record.class != Class::CredentialWriter
        || record.run != attempt.run
        || record.id != attempt.writer
        || record.generation != binding.id
        || record.lock != LockIdentity::from_fd(lock.fd())?
    {
        return Err(corrupt());
    }
    if !record.confirmed && !ledger::ownership_gone(&record)? {
        return Err(super::recovery_required());
    }
    Ok(Some(record))
}
pub(super) fn persist(dir: &Path, record: &Record) -> Result<()> {
    if resolved(dir, record)? {
        return Ok(());
    }
    let base = dir.join("credential-resolutions");
    directory(&base)?;
    write_json(
        &path(dir, record),
        &Resolution {
            generation: record.generation,
            run: record.run,
            writer: record.id,
            revision: record.revision,
            lock: record.lock.clone(),
            checked_at: state::now(),
        },
    )
}

#[cfg(test)]
#[test]
fn credential_resolution_requires_exact_fields() {
    crate::state::assert_strict_record::<Resolution>(
        &serde_json::json!({
            "generation": Uuid::nil(), "run": Uuid::nil(), "writer": Uuid::nil(),
            "revision": 1, "lock": {"dev": 1, "ino": 1}, "checked_at": 1,
        }),
        &[],
    );
}
