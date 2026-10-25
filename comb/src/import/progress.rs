use super::{
    input::Inputs,
    journal::{Batch, Entry},
};
use crate::{Error, Result, disk::Disk, values::Slug};
use comb_protocol::import::ImportState;

impl Batch {
    pub(super) fn matches(&self, inputs: &Inputs, prefix: Option<&Slug>) -> bool {
        self.source == inputs.source
            && self.prefix.as_ref() == prefix
            && self
                .entries
                .iter()
                .map(|entry| &entry.input)
                .eq(&inputs.pages)
    }
    pub(super) fn confirmed(&self, disk: &Disk) -> Result<Vec<bool>> {
        self.entries
            .iter()
            .map(|entry| entry.confirmed(disk))
            .collect::<Result<_>>()
            .map_err(Error::committed)
    }
    pub(super) fn failure(
        &self,
        mut error: Error,
        done: &[bool],
        unresolved: Option<&Slug>,
    ) -> Error {
        let mut progress = self.progress(
            done.iter().filter(|done| **done).count(),
            ImportState::Pending,
        );
        progress.unresolved = unresolved.cloned();
        tracing::warn!(name: "comb.import_pending",
             count = progress.completed,
             error = &error as &(dyn std::error::Error + 'static),
             decision = "retain_receipts");
        error.import_progress = Some(Box::new(progress));
        error.committed()
    }
}
impl Entry {
    fn confirmed(&self, disk: &Disk) -> Result<bool> {
        let expected = self.claim()?;
        let Some(saved) = disk.receipt(&expected.principal, &expected.request_id)? else {
            return Ok(false);
        };
        // Stored operation text is opaque; bind the same request identity and
        // payload as put replay without requiring the destination to still exist.
        if saved.slug != expected.slug || saved.fingerprint != expected.fingerprint {
            return Err(Error::conflicting_receipt());
        }
        Ok(true)
    }
}
