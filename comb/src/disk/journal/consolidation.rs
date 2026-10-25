//! Fixed two-page roll-forward record. Recovery uses the exact prepared bytes.
use super::super::{
    RECORD_BYTES,
    durable::{replace_checked, sync_admin, sync_existing},
};
use crate::{
    Error, Page, Result,
    disk::Disk,
    error::CommitExt,
    page::{self, Receipt},
    values::{ContentHash, Fingerprint, ReceiptOperation, RequestId, Slug},
};
use serde::{Deserialize, Serialize};

// JSON may escape one input byte as six bytes. Receipts are separately bounded
// in their encoded form; the remaining allowance covers both slugs and keys.
pub(super) const MAX_BYTES: usize =
    2 * (6 * page::PAGE_BYTES + RECORD_BYTES + 6 * crate::values::limits::SLUG_BYTES + 1024);

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreparedPage {
    pub slug: Slug,
    pub before: ContentHash,
    pub after: ContentHash,
    pub raw: String,
    pub receipt: Receipt,
}
impl PreparedPage {
    pub(crate) fn new(
        disk: &Disk,
        slug: &Slug,
        before: &ContentHash,
        mut page: Page,
        receipt: Receipt,
    ) -> Result<Self> {
        page.touch();
        disk.seal(slug, &mut page)?;
        let raw = page.render()?;
        if crate::error::value(Page::parse(&raw)?)? != crate::error::value(&page)? {
            return Err(Error::page_roundtrip_failed());
        }
        Ok(Self {
            slug: slug.clone(),
            before: before.clone(),
            after: ContentHash::digest(&raw),
            raw,
            receipt,
        })
    }
    pub(crate) fn page(&self) -> Result<Page> {
        Page::parse(&self.raw)
    }
    fn validate(&self, disk: &Disk) -> Result<()> {
        if self.raw.len() > page::PAGE_BYTES
            || crate::error::json_bytes(&self.receipt)?.len() > RECORD_BYTES
        {
            return Err(Error::file_size_limit());
        }
        if self.after != ContentHash::digest(&self.raw) || self.receipt.slug != self.slug {
            return Err(Error::invalid_consolidation_intent());
        }
        Fingerprint::try_from(self.receipt.fingerprint.clone())?;
        if !disk.valid_seal(&self.slug, &self.page()?)? {
            return Err(Error::pending_page_invalid_seal());
        }
        let saved = disk.receipt(&self.receipt.principal, &self.receipt.request_id)?;
        if saved.as_ref().is_some_and(|saved| saved != &self.receipt) {
            return Err(Error::conflicting_receipt());
        }
        Ok(())
    }
    fn current(&self, disk: &Disk) -> Result<ContentHash> {
        disk.content(&self.slug)?
            .map(ContentHash::digest)
            .ok_or_else(Error::interrupted_page_changed)
    }
    fn apply(&self, disk: &Disk) -> Result<()> {
        let path = disk.page_path(&self.slug, false)?;
        let actual = self.current(disk)?;
        if actual == self.before {
            replace_checked(
                &path,
                &disk.admin,
                self.raw.as_bytes(),
                disk.durability.as_ref(),
                &|| {
                    if self.current(disk)? != self.before {
                        return Err(Error::interrupted_page_changed());
                    }
                    Ok(())
                },
            )?;
        } else if actual != self.after {
            return Err(Error::interrupted_page_changed());
        }
        disk.sync_page_state(&self.slug, Some(&self.after))?;
        disk.set_head(&self.slug, Some(self.after.clone()))?;
        disk.set_receipt(&self.receipt.clone().into())?;
        // Equal receipt claims return early; visibility does not prove the prior
        // file and directory barriers completed.
        sync_existing(
            &disk.receipt_path(
                self.receipt.principal.as_str(),
                self.receipt.request_id.as_str(),
            )?,
            disk.durability.as_ref(),
            false,
        )
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConsolidationIntent {
    pub target: PreparedPage,
    pub note: PreparedPage,
}
impl ConsolidationIntent {
    pub(crate) fn validate(&self, disk: &Disk) -> Result<()> {
        if self.target.slug == self.note.slug
            || self.target.receipt.principal != self.note.receipt.principal
            || self.note.receipt.request_id
                != RequestId::derived(crate::error::json_bytes(&(
                    ReceiptOperation::ConsolidationSource,
                    &self.target.receipt.request_id,
                ))?)
        {
            return Err(Error::invalid_consolidation_intent());
        }
        for part in [&self.target, &self.note] {
            part.validate(disk)?;
        }
        Ok(())
    }
    pub(crate) fn before(&self, disk: &Disk) -> Result<()> {
        for part in [&self.target, &self.note] {
            if part.current(disk)? != part.before {
                return Err(Error::conflict());
            }
        }
        Ok(())
    }
    pub(crate) fn completed(&self, disk: &Disk) -> Result<()> {
        for part in [&self.target, &self.note] {
            if part.current(disk)? != part.after {
                return Err(Error::interrupted_page_changed());
            }
        }
        Ok(())
    }
    pub(super) fn apply(&self, disk: &Disk) -> Result<()> {
        self.validate(disk)?;
        for part in [&self.target, &self.note] {
            let actual = part.current(disk)?;
            if actual != part.before && actual != part.after {
                return Err(Error::interrupted_page_changed());
            }
        }
        sync_admin(&disk.admin.join("intent.json"), disk.durability.as_ref())?;
        self.target.apply(disk)?;
        #[cfg(test)]
        crate::store::interrupt_after_target()?;
        self.note.apply(disk)?;
        self.completed(disk)
    }
}

impl Disk {
    pub(crate) fn write_consolidation(&self, pair: &ConsolidationIntent) -> Result<()> {
        if super::exists(&self.admin.join("intent.json"))? {
            return Err(Error::pending_mutation());
        }
        pair.validate(self)?;
        let encoded = crate::error::json_bytes(&super::RecordRef::Consolidation(pair))?;
        if encoded.len() > MAX_BYTES {
            return Err(Error::file_size_limit());
        }
        pair.before(self)?;
        super::atomic_admin(
            &self.admin.join("intent.json"),
            &encoded,
            self.durability.as_ref(),
        )?;
        pair.apply(self).committed()
    }
}

#[cfg(test)]
mod tests;
