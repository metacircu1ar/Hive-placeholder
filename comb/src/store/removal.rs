use super::mutation::Ticket;
use super::{
    Comb, ContentHash, Error, ReceiptOperation, RequestId, Result, Slug, Write, check_expected,
};
use crate::error::CommitExt;
use crate::read::{FileFailure, Publishing};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Removed {
    pub removed: Slug,
    pub request_id: RequestId,
    pub replayed: bool,
}

impl Comb {
    pub(crate) fn replay_remove(
        &self,
        slug: &Slug,
        expected: &ContentHash,
        write: &Write<'_>,
    ) -> Result<Option<Removed>> {
        self.ready()?;
        if !write.actor.is_human() {
            return Err(Error::human_delete_only());
        }
        let ticket = Ticket::new(
            write,
            ReceiptOperation::Remove,
            (ReceiptOperation::Remove, slug, expected),
        )?;
        let Some(receipt) = self.matching_receipt(&ticket)? else {
            return Ok(None);
        };
        if receipt.slug != *slug {
            return Err(Error::receipt_slug_mismatch());
        }
        Ok(Some(Removed {
            removed: receipt.slug,
            request_id: write.request_id.clone(),
            replayed: true,
        }))
    }

    pub fn remove(
        &mut self,
        slug: &Slug,
        expected: &ContentHash,
        write: &Write<'_>,
    ) -> Result<Removed> {
        self.ready()?;
        if !write.actor.is_human() {
            return Err(Error::human_delete_only());
        }
        if let Some(reply) = self.replay_remove(slug, expected, write)? {
            return Ok(reply);
        }
        let current = self.get(slug)?;
        check_expected(Some(&current.hash), Some(expected))?;
        let receipt = Ticket::new(
            write,
            ReceiptOperation::Remove,
            (ReceiptOperation::Remove, slug, expected),
        )?
        .into_receipt(slug);
        let publication = self.publication.clone();
        let mut publishing = Publishing::file(&mut self.dirty, &publication, FileFailure::Fence);
        publishing.disk_result(
            self.disk
                .write(slug, None, Some(&current.hash), Some(receipt)),
        )?;
        #[cfg(test)]
        if let Some(hook) = self.before_index.take() {
            hook();
        }
        publishing.publish()?;
        self.index.delete(slug).committed()?;
        self.disk.finish()?;
        publishing.commit();
        Ok(Removed {
            removed: slug.clone(),
            request_id: write.request_id.clone(),
            replayed: false,
        })
    }
}
