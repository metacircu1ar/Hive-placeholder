//! Batch publication stays independent of each page's recovery intent.
use super::{
    Disk, atomic_admin,
    durable::{sync_admin, sync_existing},
    exists, read,
};
use crate::{
    Error, Result,
    import::journal::{MAX_BYTES, Record},
};

impl Disk {
    pub(crate) fn import_record(&self) -> Result<Option<Record>> {
        let path = self.admin.join("import.json");
        if !exists(&path)? {
            return Ok(None);
        }
        let record: Record = serde_json::from_slice(&read(&path, MAX_BYTES)?)?;
        record.validate(self)?;
        sync_admin(&path, self.durability.as_ref()).map_err(|error| record.failure(error, self))?;
        Ok(Some(record))
    }
    pub(crate) fn publish_import(&self, record: &Record) -> Result<()> {
        record.validate(self)?;
        let bytes = crate::error::json_bytes(record)?;
        if bytes.len() > MAX_BYTES {
            return Err(Error::file_size_limit());
        }
        atomic_admin(
            &self.admin.join("import.json"),
            &bytes,
            self.durability.as_ref(),
        )
    }
    pub(crate) fn durable_import_receipt(&self, receipt: &crate::page::Receipt) -> Result<()> {
        sync_existing(
            &self.receipt_path(receipt.principal.as_str(), receipt.request_id.as_str())?,
            self.durability.as_ref(),
            true,
        )
    }
}
