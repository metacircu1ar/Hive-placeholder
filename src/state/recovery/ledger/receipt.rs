//! Persist execution intent, authorization, completion, and immutable archival receipts.
use super::{
    Class, Error, ExecutionPhase, LedgerPhase, Lock, LockIdentity, Path, ProcessIdentity, Record,
    Result, Uuid, boot_identity, corrupt, costs, directory, exists, ownership_gone, read_json,
    recovery_required, write_json,
};
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};

pub(in crate::state::recovery) struct Receipt {
    dir: PathBuf,
    path: PathBuf,
    pub(crate) lock: Arc<Lock>,
    pub(super) record: Mutex<Record>,
}
impl Receipt {
    pub(crate) fn create(
        namespace: &super::super::namespace::Namespace,
        id: Uuid,
        class: Class,
        own_group: bool,
    ) -> Result<Self> {
        let dir = &namespace.dir;
        let lock = namespace.lock.clone();
        let _metadata = Lock::index(&dir.join("evidence.lock"))?;
        let base = dir.join(if class == Class::CredentialWriter {
            "writers"
        } else {
            "executions"
        });
        directory(&base)?;
        let path = base.join(format!("{id}.json"));
        if exists(&path)? {
            return Err(corrupt());
        }
        let record = Record {
            never_authorized: true,
            conversation: None,
            revision: 0,
            account_identity: None,
            id,
            generation: namespace.generation,
            run: namespace.run,
            boot: boot_identity()?,
            owner: ProcessIdentity::current()?,
            lock: LockIdentity::from_fd(lock.fd())?,
            class,
            phase: ExecutionPhase::Intent,
            process: None,
            own_group,
            quiescent: false,
            confirmed: false,
        };
        write_receipt(&path, &record, LedgerPhase::Create)?;
        Ok(Self {
            dir: dir.clone(),
            path,
            lock,
            record: Mutex::new(record),
        })
    }
    pub(super) fn update(
        &self,
        phase: LedgerPhase,
        change: impl FnOnce(&mut Record) -> Result<()>,
    ) -> Result<()> {
        // PoisonError carries this record's guard; the returned error must not retain it.
        let mut local = self.record.lock().map_err(|_| corrupt())?;
        let _metadata = Lock::index(&self.dir.join("evidence.lock"))?;
        let prior: Record = read_json(&self.path)?;
        if prior.id != local.id
            || prior.generation != local.generation
            || prior.run != local.run
            || prior.phase != local.phase
            || prior.revision != local.revision
        {
            return Err(corrupt());
        }
        let mut next = local.clone();
        change(&mut next)?;
        next.revision += 1;
        write_receipt(&self.path, &next, phase)?;
        *local = next;
        Ok(())
    }
    pub(crate) fn started(&self, pid: u32) -> Result<()> {
        let process = ProcessIdentity::read(pid)?.ok_or_else(recovery_required)?;
        self.update(LedgerPhase::Authorize, |r| {
            if r.phase != ExecutionPhase::Intent {
                return Err(corrupt());
            }
            r.process = Some(process);
            r.never_authorized = false;
            r.phase = ExecutionPhase::Authorized;
            Ok(())
        })
    }
    pub(crate) fn finish(&self, quiescent: bool, confirmed: bool) -> Result<()> {
        // Drop the poisoned record guard before returning the corruption error.
        let provider = self.record.lock().map_err(|_| corrupt())?.class == Class::Provider;
        self.update(LedgerPhase::Finish, |r| {
            r.quiescent = quiescent;
            r.confirmed = confirmed;
            r.phase = ExecutionPhase::Finished;
            Ok(())
        })?;
        if confirmed {
            let _metadata = Lock::index(&self.dir.join("evidence.lock"))?;
            let archive = self.dir.join("execution-archive");
            directory(&archive)?;
            // The immutable ID and generation remain in an audit receipt.
            let to = archive.join(self.path.file_name().ok_or_else(corrupt)?);
            if exists(&to)? {
                return Err(corrupt());
            }
            fs::rename(&self.path, &to).map_err(Error::state_io_from)?;
            for path in [
                archive,
                self.path.parent().ok_or_else(corrupt)?.to_path_buf(),
            ] {
                fs::File::open(path)
                    .and_then(|f| {
                        if provider {
                            costs::sync(&f, LedgerPhase::Finish, true)
                        } else {
                            crate::observability::state_sync::barrier(&f, "state", true)
                        }
                    })
                    .map_err(Error::state_io_from)?;
            }
        }
        Ok(())
    }
    pub(crate) fn confirmed(&self) -> bool {
        self.record
            .lock()
            .map_err(|_| corrupt())
            .inspect_err(|error| {
                crate::observability::observation_failed("receipt_confirmation", error);
            })
            .is_ok_and(|r| r.confirmed)
    }
    pub(crate) fn ownership_gone(&self) -> Result<bool> {
        // Drop the poisoned record guard before returning the corruption error.
        let record = self.record.lock().map_err(|_| corrupt())?;
        Ok(record.confirmed || ownership_gone(&record)?)
    }
    pub(crate) fn was_started(&self) -> bool {
        self.record
            .lock()
            .map_err(|_| corrupt())
            .inspect_err(|error| crate::observability::observation_failed("receipt_started", error))
            .is_ok_and(|record| record.process.is_some())
    }
}

fn write_receipt(path: &Path, record: &Record, phase: LedgerPhase) -> Result<()> {
    if record.class == Class::Provider {
        costs::write_json(path, record, phase)
    } else {
        write_json(path, record)
    }
}
