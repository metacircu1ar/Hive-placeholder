//! Authenticated page heads and opaque durable receipt claims.
use super::{
    Disk, RECORD_BYTES, durable::replace_ordered, exists, filesystem::same_device, read,
    required_option,
};
use crate::{
    Error, Result, page,
    values::{ContentHash, Principal, RequestId, Slug},
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HeadRecord {
    slug: String,
    #[serde(deserialize_with = "required_option")]
    hash: Option<String>,
}

#[derive(Serialize)]
struct Head {
    slug: Slug,
    hash: Option<ContentHash>,
}

impl Disk {
    pub(super) fn receipt_path(&self, principal: &str, request_id: &str) -> Result<PathBuf> {
        same_device(&self.admin.join("receipts"), self.root_device)?;
        Ok(self.admin.join("receipts").join(format!(
            "{}.json",
            page::hash(crate::error::json_bytes(&(principal, request_id))?)
        )))
    }
    pub(super) fn receipt_claim(
        &self,
        principal: &str,
        request_id: &str,
    ) -> Result<Option<(page::ReceiptRecord, Slug)>> {
        let path = self.receipt_path(principal, request_id)?;
        if !exists(&path)? {
            return Ok(None);
        }
        let receipt: page::ReceiptRecord = serde_json::from_slice(&read(&path, RECORD_BYTES)?)?;
        if receipt.principal != principal || receipt.request_id != request_id {
            return Err(Error::receipt_identity_mismatch());
        }
        // Stored claims have always checked the slug after identity, even during recovery.
        let slug = Slug::try_from(receipt.slug.clone())?;
        Ok(Some((receipt, slug)))
    }
    pub fn receipt(
        &self,
        principal: &Principal,
        request_id: &RequestId,
    ) -> Result<Option<page::Receipt>> {
        Ok(self
            .receipt_claim(principal.as_str(), request_id.as_str())?
            .map(|(receipt, slug)| page::Receipt {
                principal: principal.clone(),
                request_id: request_id.clone(),
                fingerprint: receipt.fingerprint,
                operation: receipt.operation,
                slug,
            }))
    }
    pub(super) fn set_receipt(&self, receipt: &page::ReceiptRecord) -> Result<()> {
        if let Some((existing, _)) = self.receipt_claim(&receipt.principal, &receipt.request_id)? {
            if existing != *receipt {
                return Err(Error::conflicting_receipt());
            }
            return Ok(());
        }
        replace_ordered(
            &self.receipt_path(&receipt.principal, &receipt.request_id)?,
            &crate::error::json_bytes(receipt)?,
            self.durability.as_ref(),
        )
    }
    pub(super) fn head_path(&self, slug: &Slug) -> PathBuf {
        self.admin
            .join("heads")
            .join(format!("{}.json", page::hash(slug.as_str())))
    }
    pub fn head(&self, slug: &Slug) -> Result<Option<ContentHash>> {
        same_device(&self.admin.join("heads"), self.root_device)?;
        let path = self.head_path(slug);
        if !exists(&path)? {
            return Ok(None);
        }
        let head: HeadRecord = serde_json::from_slice(&read(&path, super::limits::HEAD_BYTES)?)?;
        if head.slug != slug.as_str() {
            return Err(Error::receipt_slug_mismatch());
        }
        head.hash.map(ContentHash::try_from).transpose()
    }
    pub(super) fn set_head(&self, slug: &Slug, hash: Option<ContentHash>) -> Result<()> {
        same_device(&self.admin.join("heads"), self.root_device)?;
        replace_ordered(
            &self.head_path(slug),
            &crate::error::json_bytes(&Head {
                slug: slug.clone(),
                hash,
            })?,
            self.durability.as_ref(),
        )
    }
}
