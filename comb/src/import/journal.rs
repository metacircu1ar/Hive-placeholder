use super::input::{Input, PATH_BYTES, Source};
use crate::{
    Error, Result,
    disk::Disk,
    page::Receipt,
    store::{
        import_receipt,
        limits::{IMPORT_BYTES, IMPORT_PAGES},
    },
    values::{CombId, ContentHash, Fingerprint, ImportId, RequestId, Slug},
};
use comb_protocol::import::{ImportProgress, ImportReport, ImportState};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

// Two path spellings per input plus bounded receipts and fields; Markdown's
// existing aggregate bound may expand sixfold when encoded as JSON.
pub(crate) const MAX_BYTES: usize =
    6 * IMPORT_BYTES + IMPORT_PAGES * (12 * PATH_BYTES + 4096) + 12 * PATH_BYTES;

#[derive(Debug, Serialize, Deserialize)]
#[serde(
    tag = "state",
    content = "batch",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(crate) enum Record {
    Pending(Box<Batch>),
    Completed(Box<Batch>),
    Abandoned(Box<Abandoned>),
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Batch {
    pub(super) comb: CombId,
    pub(super) id: ImportId,
    pub(super) source: Source,
    #[serde(deserialize_with = "required")]
    pub(super) prefix: Option<Slug>,
    pub(super) entries: Vec<Entry>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Entry {
    pub(super) input: Input,
    #[serde(deserialize_with = "required")]
    pub(super) expected: Option<ContentHash>,
    pub(super) request_id: RequestId,
    pub(super) fingerprint: Fingerprint,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Abandoned {
    pub(super) comb: CombId,
    pub(super) progress: ImportProgress,
}
impl Record {
    pub(crate) fn validate(&self, disk: &Disk) -> Result<()> {
        match self {
            Self::Pending(batch) => batch.validate(disk),
            Self::Completed(batch) => {
                batch.validate(disk)?;
                if batch.confirmed(disk)?.iter().any(|done| !done) {
                    return Err(Error::invalid_import_journal().committed());
                }
                Ok(())
            }
            Self::Abandoned(record) => {
                record.progress.validate()?;
                if record.comb != disk.id() || record.progress.state != ImportState::Abandoned {
                    return Err(Error::invalid_import_journal());
                }
                Ok(())
            }
        }
    }
}
impl Batch {
    fn validate(&self, disk: &Disk) -> Result<()> {
        if self.comb != disk.id()
            || self.entries.len() > IMPORT_PAGES
            || !self.source.directory.is_absolute()
            || self
                .source
                .directory
                .to_str()
                .is_none_or(|s| s.len() > PATH_BYTES)
            || self
                .entries
                .windows(2)
                .any(|pair| pair[0].input.relative >= pair[1].input.relative)
        {
            return Err(Error::invalid_import_journal());
        }
        let mut total = 0usize;
        let mut slugs = BTreeSet::new();
        for entry in &self.entries {
            total = total
                .checked_add(entry.input.markdown.len())
                .ok_or_else(Error::import_byte_limit)?;
            if total > IMPORT_BYTES {
                return Err(Error::import_byte_limit());
            }
            if !slugs.insert(&entry.input.slug)
                || entry.input.slug
                    != super::input::slug(&entry.input.relative, self.prefix.as_ref())?
                || entry.request_id != page_request(&self.id, &entry.input.relative)?
                || entry.claim()?.fingerprint != entry.fingerprint.as_str()
                || disk
                    .root
                    .join(format!("{}.md", entry.input.slug))
                    .starts_with(&self.source.directory)
            {
                return Err(Error::invalid_import_journal());
            }
            crate::Page::parse(&entry.input.markdown)?;
        }
        Ok(())
    }
    pub(super) fn progress(&self, completed: usize, state: ImportState) -> ImportProgress {
        ImportProgress {
            batch_id: self.id.clone(),
            state,
            total: self.entries.len(),
            completed,
            unfinished: self.entries.len() - completed,
            unresolved: None,
        }
    }
    pub(super) fn report(&self) -> ImportReport {
        ImportReport {
            batch_id: self.id.clone(),
            total: self.entries.len(),
            completed: self
                .entries
                .iter()
                .map(|entry| entry.input.slug.clone())
                .collect(),
            state: ImportState::Completed,
        }
    }
}
impl Entry {
    pub(super) fn claim(&self) -> Result<Receipt> {
        import_receipt(
            &self.input.slug,
            &self.input.markdown,
            self.expected.as_ref(),
            &self.request_id,
        )
    }
}
pub(super) fn page_request(id: &ImportId, relative: &str) -> Result<RequestId> {
    Ok(RequestId::derived(crate::error::json_bytes(&(
        "import-page",
        id,
        relative,
    ))?))
}
fn required<'de, T: Deserialize<'de>, D: serde::Deserializer<'de>>(
    input: D,
) -> std::result::Result<Option<T>, D::Error> {
    Option::deserialize(input)
}
