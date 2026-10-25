//! Inspect execution ownership and produce recovery reservations and reconciliation receipts.
use super::{
    super::report::{Completion, Observation, Reconciliation},
    Binding, Class, Error, ExecutionPhase, Lock, LockIdentity, Path, Record, RecoveryReservation,
    Result, State, Uuid, check_path, corrupt, directory, exists, read_json, recovery_required,
    same_boot, write_json,
};
use crate::{os, runtime::api::value as project, state};
use std::fs;

pub(in crate::state::recovery) fn records(dir: &Path) -> Result<Vec<Record>> {
    let mut result = records_in(dir, "executions")?;
    result.extend(records_in(dir, "writers")?);
    Ok(result)
}
pub(in crate::state::recovery) fn records_in(dir: &Path, name: &str) -> Result<Vec<Record>> {
    super::layout::audit_profile(dir)?;
    let base = dir.join(name);
    if !exists(&base)? {
        return Ok(Vec::new());
    }
    let _metadata = Lock::index(&dir.join("evidence.lock"))?;
    records_owned(dir, name)
}
/// Caller owns exclusive root admission and the original auth lock. Reading
/// retained evidence must not create or repair an evidence lock during dry-run.
pub(in crate::state::recovery) fn records_owned(dir: &Path, name: &str) -> Result<Vec<Record>> {
    super::layout::audit_profile(dir)?;
    let base = dir.join(name);
    if !exists(&base)? {
        return Ok(Vec::new());
    }
    check_path(&base, true)?;
    let mut records = Vec::new();
    for entry in fs::read_dir(base).map_err(Error::state_io_from)? {
        let entry = entry.map_err(Error::state_io_from)?;
        if state::staging_file(&entry.path())? {
            continue;
        }
        let r: Record = read_json(&entry.path())?;
        if entry.file_name().to_str() != Some(&format!("{}.json", r.id)) {
            return Err(corrupt());
        }
        records.push(r);
    }
    Ok(records)
}
pub(in crate::state) fn migration_records(
    dir: &Path,
    generation: Uuid,
    lock: &LockIdentity,
) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    for name in ["writers", "executions", "execution-archive"] {
        for record in records_owned(dir, name)? {
            if record.generation != generation || &record.lock != lock || !seen.insert(record.id) {
                return Err(corrupt());
            }
            if !migration_safe(&record)? {
                return Err(recovery_required());
            }
        }
    }
    Ok(())
}
pub(in crate::state::recovery) fn migration_safe(record: &Record) -> Result<bool> {
    // A confirmed never-authorized intent has no child, even if its creating
    // process continues to run. Actual launches still need the native proof.
    Ok(
        (record.confirmed && record.never_authorized && record.process.is_none())
            || ownership_gone(record)?,
    )
}
/// Caller holds evidence.lock; duplicate active/archive receipts are corruption.
pub(in crate::state::recovery) fn find_record(
    dir: &Path,
    bases: [&str; 2],
    id: Uuid,
) -> Result<Option<Record>> {
    let mut found = None;
    for base in bases {
        let base = dir.join(base);
        if exists(&base)? {
            check_path(&base, true)?;
        }
        let path = base.join(format!("{id}.json"));
        if exists(&path)? {
            if found.is_some() {
                return Err(corrupt());
            }
            found = Some(read_json::<Record>(&path)?);
        }
    }
    Ok(found)
}
fn native_never_authorized(record: &Record) -> Result<bool> {
    Ok(
        (record.phase == ExecutionPhase::Intent || record.never_authorized)
            && record.process.is_none()
            && record.owner.absent()?,
    )
}
pub(in crate::state::recovery) fn ownership_gone(record: &Record) -> Result<bool> {
    if !same_boot(&record.boot)? {
        return Ok(true);
    }
    if native_never_authorized(record)? {
        return Ok(true);
    }
    let Some(process) = &record.process else {
        return Ok(false);
    };
    Ok(process.absent()?
        && (!record.own_group || os::group_absent(process.pid).map_err(Error::state_io_from)?))
}
pub(in crate::state::recovery) fn conversation_proven(record: &Record) -> Result<bool> {
    Ok(record.confirmed
        || native_never_authorized(record)?
        || (record.quiescent && ownership_gone(record)?))
}
pub(in crate::state) fn check_writers(dir: &Path, generation: Uuid) -> Result<()> {
    check_writers_for_operation(dir, generation, None, false)
}
pub(in crate::state::recovery) fn check_writers_for_operation(
    dir: &Path,
    generation: Uuid,
    pending: Option<&Record>,
    pruning: bool,
) -> Result<()> {
    for record in records_in(dir, "writers")? {
        if record.generation != generation || record.class != Class::CredentialWriter {
            return Err(corrupt());
        }
        let safe = record.confirmed
            || native_never_authorized(&record)?
            || super::credential::resolved(dir, &record)?
            || pending.is_some_and(|pending| {
                pending.id == record.id
                    && pending.revision == record.revision
                    && pending.run == record.run
            })
            || (pruning && ownership_gone(&record)?);
        if !safe {
            return Err(recovery_required());
        }
    }
    Ok(())
}
pub(in crate::state::recovery) fn conversation_safe(
    binding: &Binding,
    provider: &str,
) -> Result<bool> {
    let dir = binding.directory.clone();
    super::layout::profile(&dir)?;
    let id = Uuid::parse_str(provider).map_err(|e| corrupt().with_source(e))?;
    let snapshot = {
        let _metadata = Lock::index(&dir.join("evidence.lock"))?;
        find_record(&dir, ["executions", "execution-archive"], id)?
    };
    if let Some(record) = snapshot {
        if record.class != Class::Provider
            || record.id != id
            || record.generation != binding.id
            || record.lock != LockIdentity::at(&dir.join("auth.lock"))?
        {
            return Err(corrupt());
        }
        return conversation_proven(&record);
    }
    Ok(false)
}
pub(in crate::state::recovery) fn reservations(
    binding: &Binding,
    name: &str,
) -> Vec<RecoveryReservation> {
    match records(&binding.directory) {
        Ok(rows) => rows
            .into_iter()
            .filter(|r| r.class == Class::Provider && !r.confirmed)
            .map(|r| {
                let gone = ownership_gone(&r)
                    .inspect_err(|error| {
                        let provider_id = r.id.to_string();
                        tracing::warn!(name: "recovery.reservation_uncertain",
                         provider_id = provider_id,
                         error = error as &(dyn std::error::Error + 'static),
                         decision = "retain_capacity");
                    })
                    .unwrap_or(false);
                RecoveryReservation {
                    auth: name.into(),
                    provider: r.id.to_string(),
                    process: r.process,
                    gone,
                    unknown_capacity: false,
                    unresolved_activity: !r.quiescent,
                }
            })
            .collect(),
        Err(error) => {
            tracing::warn!(name: "recovery.reservation_uncertain",
                 error = &error as &(dyn std::error::Error + 'static),
                 decision = "retain_unknown_capacity");
            vec![RecoveryReservation {
                auth: name.into(),
                provider: String::new(),
                process: None,
                gone: false,
                unknown_capacity: true,
                unresolved_activity: true,
            }]
        }
    }
}
pub(in crate::state::recovery) fn reconcile(
    state: &State,
    binding: &Binding,
) -> Result<Reconciliation> {
    if !binding.ready {
        return Err(Error::registration_incomplete(
            state::REGISTRATION_INCOMPLETE,
        ));
    }
    let dir = binding.directory.clone();
    super::layout::profile(&dir)?;
    let lock = Lock::shared(&dir.join("auth.lock"))?;
    let account = state.verify_credential(binding, &lock)?;
    let (rows, history_error) = match records(&dir) {
        Ok(rows) => (rows, None),
        Err(error) => (Vec::new(), Some(error)),
    };
    // Kernel observations happen outside the metadata lock. They cannot signal.
    let mut observations = Vec::new();
    for row in &rows {
        if row.generation != binding.id {
            return Err(corrupt());
        }
        observations.push(observe(row)?);
    }
    let usable = check_writers(&dir, binding.id)
        .inspect_err(|error| {
            let profile_id = binding.id.to_string();
            tracing::warn!(name: "auth.generation_unusable",
             profile_id = profile_id,
             error = error as &(dyn std::error::Error + 'static),
             decision = "retain_evidence");
        })
        .is_ok();
    let mut receipt = Reconciliation {
        generation: binding.id,
        checked_at: state::now(),
        credential_lock: LockIdentity::from_fd(lock.fd())?,
        runtime_usable: usable,
        account_identity_available: account.is_some(),
        history_error,
        executions: observations,
        snapshot_current: false,
    };
    let _metadata = Lock::index(&dir.join("evidence.lock"))?;
    let current = |row: &Record| -> bool {
        let base = if row.class == Class::CredentialWriter {
            "writers"
        } else {
            "executions"
        };
        read_json::<Record>(&dir.join(base).join(format!("{}.json", row.id)))
            .inspect_err(|error| {
                let execution_id = row.id.to_string();
                tracing::warn!(name: "recovery.snapshot_unavailable",
                 execution_id = execution_id,
                 error = error as &(dyn std::error::Error + 'static),
                 decision = "mark_snapshot_incomplete");
            })
            .is_ok_and(|r| {
                r.id == row.id
                    && r.generation == row.generation
                    && r.run == row.run
                    && r.revision == row.revision
            })
    };
    receipt.snapshot_current = rows.iter().all(current);
    let receipts = dir.join("recovery-receipts");
    directory(&receipts)?;
    write_json(
        &receipts.join(format!("{}.json", Uuid::new_v4())),
        &project(&receipt),
    )?;
    Ok(receipt)
}

fn observe(row: &Record) -> Result<Observation> {
    let ownership_gone = ownership_gone(row)?;
    let conversation_safe = conversation_proven(row)?;
    Ok(Observation {
        class: row.class,
        completion: Completion {
            confirmed: row.confirmed,
            quiescent: row.quiescent,
        },
        conversation_safe,
        execution_id: row.id,
        ownership_gone,
        phase: row.phase,
        revision: row.revision,
    })
}
