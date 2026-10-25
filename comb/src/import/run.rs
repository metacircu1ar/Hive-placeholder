use super::{
    input::Inputs,
    journal::{Batch, Entry, Record, page_request},
};
use crate::{
    Actor, Comb, Error, Result,
    store::{Write, import_receipt},
    values::{Fingerprint, ImportId, Slug},
};
use comb_protocol::import::ImportReport;

impl Comb {
    pub(super) fn prepare_import(
        &self,
        inputs: Inputs,
        prefix: Option<&Slug>,
    ) -> Result<Box<Batch>> {
        let id = ImportId::generate();
        let mut entries = Vec::new();
        for input in inputs.pages {
            let destination = self.disk.page_path(&input.slug, false)?;
            if destination.starts_with(&inputs.source.directory) {
                return Err(Error::import_overlap());
            }
            let expected = match self.get(&input.slug) {
                Ok(page) => Some(page.hash),
                Err(error) if error.is_not_found() => None,
                Err(error) => return Err(error),
            };
            let request_id = page_request(&id, &input.relative)?;
            self.preflight_import_page(
                &input.slug,
                &input.markdown,
                expected.as_ref(),
                &request_id,
            )?;
            let claim =
                import_receipt(&input.slug, &input.markdown, expected.as_ref(), &request_id)?;
            if self
                .disk
                .receipt(&claim.principal, &claim.request_id)?
                .is_some()
            {
                return Err(Error::conflicting_receipt());
            }
            entries.push(Entry {
                input,
                expected,
                request_id,
                fingerprint: Fingerprint::try_from(claim.fingerprint)?,
            });
        }
        Ok(Box::new(Batch {
            comb: self.identity(),
            id,
            source: inputs.source,
            prefix: prefix.cloned(),
            entries,
        }))
    }
    pub(super) fn resume_import(
        &mut self,
        batch: Box<Batch>,
        mut done: Vec<bool>,
    ) -> Result<ImportReport> {
        tracing::info!(name: "comb.import_started", count = batch.entries.len());
        self.preflight_remaining(&batch, &done)?;
        for (index, entry) in batch.entries.iter().enumerate() {
            if done[index] {
                continue;
            }
            let write = Write::new(&Actor::Human, &entry.request_id, None)?;
            self.put_markdown(
                &entry.input.slug,
                &entry.input.markdown,
                entry.expected.as_ref(),
                &write,
            )
            .map_err(|error| batch.failure(error, &done, Some(&entry.input.slug)))?;
            done[index] = true;
            tracing::debug!(name: "comb.import_page_committed", count = index + 1);
        }
        for entry in &batch.entries {
            let claim = entry
                .claim()
                .map_err(|error| batch.failure(error, &done, None))?;
            self.disk
                .durable_import_receipt(&claim)
                .map_err(|error| batch.failure(error, &done, None))?;
        }
        let report = batch.report();
        let completed = Record::Completed(batch);
        if let Err(mut error) = self.disk.publish_import(&completed) {
            tracing::warn!(name: "comb.import_pending",
                 count = report.total,
                 error = &error as &(dyn std::error::Error + 'static),
                 decision = "retain_receipts");
            error.import_progress = Some(Box::new(comb_protocol::import::ImportProgress {
                batch_id: report.batch_id.clone(),
                state: comb_protocol::import::ImportState::Pending,
                total: report.total,
                completed: report.total,
                unfinished: 0,
                unresolved: None,
            }));
            return Err(error.committed());
        }
        tracing::info!(name: "comb.import_finished", count = report.total, success = true);
        Ok(report)
    }
    fn preflight_remaining(&self, batch: &Batch, done: &[bool]) -> Result<()> {
        for (entry, done_entry) in batch.entries.iter().zip(done) {
            if *done_entry {
                continue;
            }
            self.preflight_import_page(
                &entry.input.slug,
                &entry.input.markdown,
                entry.expected.as_ref(),
                &entry.request_id,
            )
            .map_err(|error| {
                let error = if error.code == crate::ErrorCode::Conflict {
                    Error::import_target_changed().caused_by(error)
                } else {
                    error
                };
                batch.failure(error, done, Some(&entry.input.slug))
            })?;
        }
        Ok(())
    }
}
