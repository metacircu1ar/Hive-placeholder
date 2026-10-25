//! Intent records, durable file-first writes and recovery.
use super::{
    Disk, RECORD_BYTES, atomic_admin,
    durable::{create_ordered_checked, replace_checked},
    exists, read, required_option,
};
use crate::{
    Error, Page, Result,
    error::CommitExt,
    page,
    values::{ContentHash, Slug},
};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};

mod consolidation;
pub(crate) use consolidation::{ConsolidationIntent, PreparedPage};

#[derive(Deserialize)]
#[serde(
    tag = "operation",
    content = "record",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum Record {
    Single(Box<IntentRecord>),
    Consolidation(Box<ConsolidationIntent>),
}
#[derive(Serialize)]
#[serde(tag = "operation", content = "record", rename_all = "snake_case")]
enum RecordRef<'a> {
    Single(&'a Intent),
    Consolidation(&'a ConsolidationIntent),
}
#[derive(Debug)]
pub(crate) enum Recovery {
    Single(Slug),
    Consolidation(Box<ConsolidationIntent>),
}

#[cfg(test)]
pub(super) const POPULATION: &[u8] = b"{\"operation\":\"populate_index\",\"space\":null}";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Population {
    operation: PopulationOperation,
    #[serde(deserialize_with = "required_option")]
    space: Option<crate::embedding::Space>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PopulationOperation {
    PopulateIndex,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IntentRecord {
    slug: String,
    #[serde(deserialize_with = "required_option")]
    before: Option<String>,
    #[serde(deserialize_with = "required_option")]
    previous_head: Option<String>,
    #[serde(deserialize_with = "required_option")]
    after: Option<String>,
    #[serde(deserialize_with = "required_option")]
    receipt: Option<page::ReceiptRecord>,
}

#[derive(Serialize)]
struct Intent {
    slug: Slug,
    before: Option<ContentHash>,
    previous_head: Option<ContentHash>,
    after: Option<ContentHash>,
    receipt: Option<page::ReceiptRecord>,
}
impl TryFrom<IntentRecord> for Intent {
    type Error = Error;
    fn try_from(raw: IntentRecord) -> Result<Self> {
        let before = raw.before.map(ContentHash::try_from).transpose()?;
        let previous_head = raw.previous_head.map(ContentHash::try_from).transpose()?;
        let after = raw.after.map(ContentHash::try_from).transpose()?;
        let slug = Slug::try_from(raw.slug)?;
        Ok(Self {
            slug,
            before,
            previous_head,
            after,
            receipt: raw.receipt,
        })
    }
}

impl Disk {
    pub fn index_population(&self, start: bool) -> Result<bool> {
        if self.population()?.is_some() {
            return Ok(true);
        }
        if start {
            self.start_population(None)?;
        }
        Ok(start)
    }
    fn population(&self) -> Result<Option<Population>> {
        let path = self.admin.join("index-population.json");
        if !exists(&path)? {
            return Ok(None);
        }
        let bytes = read(&path, super::limits::RECORD_BYTES)?;
        if bytes.first() != Some(&b'{') {
            return Err(Error::invalid_population_marker());
        }
        let record: Population = serde_json::from_slice(&bytes)?;
        if let Some(space) = &record.space {
            space.validate()?;
        }
        Ok(Some(record))
    }
    pub(super) fn start_population(&self, space: Option<crate::embedding::Space>) -> Result<()> {
        if let Some(existing) = self.population()? {
            if existing.space != space {
                return Err(Error::invalid_population_marker());
            }
            return Ok(());
        }
        let record = Population {
            operation: PopulationOperation::PopulateIndex,
            space,
        };
        let bytes = crate::error::json_bytes(&record)?;
        if bytes.len() > super::limits::RECORD_BYTES {
            return Err(Error::intent_size_limit());
        }
        atomic_admin(
            &self.admin.join("index-population.json"),
            &bytes,
            self.durability.as_ref(),
        )
    }
    pub(crate) fn population_space(&self) -> Result<Option<crate::embedding::Space>> {
        Ok(self.population()?.and_then(|p| p.space))
    }
    pub fn finish_population(&self) -> Result<()> {
        self.finish_marker("index-population.json")
    }
    pub fn write(
        &self,
        slug: &Slug,
        page: Option<&Page>,
        expected: Option<&ContentHash>,
        receipt: Option<page::Receipt>,
    ) -> Result<()> {
        if exists(&self.admin.join("intent.json"))? {
            return Err(Error::pending_mutation());
        }
        let path = self.page_path(slug, true)?;
        #[cfg(test)]
        super::durable::checkpoint(
            &path,
            hive_setup_support::checkpoints::Checkpoint::BeforeIntent,
        )?;
        let before = self.content(slug)?.map(ContentHash::digest);
        if before.as_ref() != expected {
            return Err(Error::conflict());
        }
        let after = page.map(Page::render).transpose()?;
        // Markdown has structural delimiters. Never commit input whose rendered
        // form would swallow, invent or alter an evidence entry on the next read.
        if let (Some(page), Some(raw)) = (page, &after)
            && crate::error::value(Page::parse(raw)?)? != crate::error::value(page)?
        {
            return Err(Error::invalid_markdown_structure());
        }
        let intent = Intent {
            slug: slug.clone(),
            before,
            previous_head: self.head(slug)?,
            after: after.as_ref().map(ContentHash::digest),
            receipt: receipt.map(Into::into),
        };
        let encoded = crate::error::json_bytes(&RecordRef::Single(&intent))?;
        if encoded.len() > RECORD_BYTES {
            return Err(Error::intent_size_limit());
        }
        atomic_admin(
            &self.admin.join("intent.json"),
            &encoded,
            self.durability.as_ref(),
        )?;
        let check = || {
            if self.content(slug)?.map(ContentHash::digest) != intent.before {
                return Err(Error::interrupted_page_changed());
            }
            Ok(())
        };
        if let Some(bytes) = &after {
            let publish = if intent.before.is_none() {
                create_ordered_checked
            } else {
                replace_checked
            };
            publish(
                &path,
                &self.admin,
                bytes.as_bytes(),
                self.durability.as_ref(),
                &check,
            )
            .committed()?;
        } else {
            #[cfg(test)]
            super::durable::checkpoint(&path, atomic_fs::Step::BeforeRemove).committed()?;
            check().committed()?;
            fs::remove_file(&path).committed()?;
            self.durability
                .fsync(&File::open(path.parent().ok_or_else(Error::parent_missing)?).committed()?)
                .committed()?;
        }
        self.set_head(slug, intent.after).committed()?;
        if let Some(receipt) = &intent.receipt {
            self.set_receipt(receipt).committed()?;
        }
        Ok(())
    }
    pub fn finish(&self) -> Result<()> {
        self.durability.before_finish()?;
        self.finish_marker("intent.json")
    }
    fn finish_marker(&self, name: &str) -> Result<()> {
        let path = self.admin.join(name);
        // All page/head/receipt/index files and parent entries have been fsynced.
        // Flush device caches while recovery evidence still exists. A power loss
        // after this barrier may retain the marker, but replay is harmless.
        #[cfg(test)]
        super::durable::checkpoint(&path, atomic_fs::Step::BeforeFileSync).committed()?;
        self.durability
            .full_flush(&File::open(&path).committed()?)
            .committed()?;
        #[cfg(test)]
        super::durable::checkpoint(&path, atomic_fs::Step::AfterFileSync).committed()?;
        #[cfg(test)]
        super::durable::checkpoint(&path, atomic_fs::Step::BeforeRemove).committed()?;
        fs::remove_file(&path).committed()?;
        #[cfg(test)]
        super::durable::checkpoint(&path, atomic_fs::Step::AfterRemove).committed()?;
        #[cfg(test)]
        super::durable::checkpoint(&path, atomic_fs::Step::BeforeDirectorySync).committed()?;
        self.durability
            .fsync(&File::open(&self.admin).committed()?)
            .committed()?;
        #[cfg(test)]
        super::durable::checkpoint(&path, atomic_fs::Step::AfterDirectorySync).committed()?;
        Ok(())
    }
    pub fn recover(&self) -> Result<Option<Recovery>> {
        let path = self.admin.join("intent.json");
        if !exists(&path)? {
            return Ok(None);
        }
        let encoded = read(&path, consolidation::MAX_BYTES)?;
        match serde_json::from_slice::<Record>(&encoded)? {
            Record::Single(record) => {
                if encoded.len() > RECORD_BYTES {
                    return Err(Error::file_size_limit());
                }
                let intent = Intent::try_from(*record)?;
                self.recover_single(&intent).committed()?;
                Ok(Some(Recovery::Single(intent.slug)))
            }
            Record::Consolidation(pair) => {
                pair.apply(self).committed()?;
                Ok(Some(Recovery::Consolidation(pair)))
            }
        }
    }
    fn recover_single(&self, intent: &Intent) -> Result<()> {
        let raw = self.content(&intent.slug)?;
        let actual = raw.as_ref().map(ContentHash::digest);
        if actual != intent.after && actual != intent.before {
            return Err(Error::interrupted_page_changed());
        }
        self.sync_page_state(&intent.slug, actual.as_ref())?;
        if actual == intent.after {
            if let Some(raw) = raw
                && !self.valid_seal(&intent.slug, &Page::parse(&raw)?)?
            {
                return Err(Error::pending_page_invalid_seal());
            }
            self.set_head(&intent.slug, intent.after.clone())?;
            if let Some(receipt) = &intent.receipt {
                if receipt.slug != intent.slug.as_str() {
                    return Err(Error::pending_receipt_wrong_page());
                }
                self.set_receipt(receipt)?;
                super::durable::sync_existing(
                    &self.receipt_path(&receipt.principal, &receipt.request_id)?,
                    self.durability.as_ref(),
                    false,
                )?;
            }
        } else if actual == intent.before {
            // Before the final device barrier, sidecars may reach persistent
            // media ahead of the page. Restore authority and retract the exact
            // tentative receipt rather than acknowledging an unapplied request.
            self.set_head(&intent.slug, intent.previous_head.clone())?;
            if let Some(receipt) = &intent.receipt
                && let Some((saved, _)) =
                    self.receipt_claim(&receipt.principal, &receipt.request_id)?
            {
                if saved != *receipt {
                    return Err(Error::pending_receipt_conflict());
                }
                fs::remove_file(self.receipt_path(&receipt.principal, &receipt.request_id)?)?;
                self.durability
                    .fsync(&File::open(self.admin.join("receipts"))?)?;
            }
        } else {
            return Err(Error::interrupted_page_changed());
        }
        Ok(())
    }
}
