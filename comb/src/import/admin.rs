use super::journal::{Abandoned, Record};
use crate::{Comb, Error, Result, values::ImportId};
use comb_protocol::import::{ImportProgress, ImportState, ImportStatus};

impl Comb {
    pub fn import_status(&self) -> Result<ImportStatus> {
        self.ready()?;
        Ok(ImportStatus {
            current: self
                .disk
                .import_record()?
                .map(|record| record.progress(&self.disk))
                .transpose()?,
        })
    }
    pub fn abandon_import(&self, id: &ImportId) -> Result<ImportProgress> {
        self.ready()?;
        let record = self
            .disk
            .import_record()?
            .ok_or_else(Error::import_batch_not_current)?;
        let mut progress = record.progress(&self.disk)?;
        if &progress.batch_id != id {
            return Err(Error::import_batch_not_current());
        }
        if progress.state != ImportState::Pending {
            return Ok(progress);
        }
        progress.state = ImportState::Abandoned;
        progress.unresolved = None;
        let record = Record::Abandoned(Box::new(Abandoned {
            comb: self.identity(),
            progress: progress.clone(),
        }));
        self.disk.publish_import(&record).map_err(|mut error| {
            // The prior accepted pending batch is preserved on prepublication
            // failure; --status or the same exact ID resolves an uncertain reply.
            let mut uncertain = progress.clone();
            uncertain.state = ImportState::Pending;
            error.import_progress = Some(Box::new(uncertain));
            error
        })?;
        Ok(progress)
    }
}
impl Record {
    pub(crate) fn failure(&self, mut error: Error, disk: &crate::disk::Disk) -> Error {
        // Preserve the original barrier failure even if corrupt receipts prevent
        // us from attaching trustworthy progress to the accepted batch.
        match self.progress(disk) {
            Ok(progress) => error.import_progress = Some(Box::new(progress)),
            Err(secondary) => {
                tracing::warn!(name: "comb.import_progress_unavailable",
                     primary = &error as &(dyn std::error::Error + 'static),
                     secondary = &secondary as &(dyn std::error::Error + 'static),
                     decision = "retain_primary_and_receipts");
            }
        }
        error.committed()
    }
    fn progress(&self, disk: &crate::disk::Disk) -> Result<ImportProgress> {
        match self {
            Self::Pending(batch) => {
                let done = batch.confirmed(disk)?;
                Ok(batch.progress(
                    done.iter().filter(|done| **done).count(),
                    ImportState::Pending,
                ))
            }
            Self::Completed(batch) => {
                Ok(batch.progress(batch.entries.len(), ImportState::Completed))
            }
            Self::Abandoned(record) => Ok(record.progress.clone()),
        }
    }
}
