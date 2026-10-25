//! Charge before any possible native submission; never reclaim history implicitly.
mod queries;
use super::{Message, Queries, Store};
use crate::{
    error::{Error, Result},
    runtime::attachments::{
        Delivery,
        images::{self, Usage},
    },
};
use rusqlite::params;

impl Queries {
    pub(in crate::runtime) fn image_usage(&self, agent: &str) -> Result<Usage> {
        Ok(self
            .query_opt_cached(queries::IMAGE_USAGE_SELECT, [agent], |r| {
                Ok(Usage {
                    bytes: super::rows::unsigned(r, 0)?,
                    images: r.get(1)?,
                })
            })?
            .unwrap_or_default())
    }
}
impl Store {
    pub(in crate::runtime) fn save_claimed_attachment_delivery(
        &self,
        agent: &str,
        id: &str,
        items: &[crate::runtime::attachments::Attachment],
    ) -> Result<()> {
        let claimed: bool = self.query_row_cached(
            queries::SAVE_CLAIMED_ATTACHMENT_DELIVERY_SELECT,
            params![id, agent],
            |r| r.get(0),
        )?;
        if !claimed {
            return Err(Error::stale_execution(
                "Attachment input is no longer claimed.",
            ));
        }
        self.save_attachment_delivery(id, items)
    }
    pub(in crate::runtime) fn plan_message_images(&self, message: &mut Message) -> Result<()> {
        if message.attachments.is_empty() {
            return Ok(());
        }
        let pending = message
            .attachments
            .iter()
            .filter(|a| a.delivery == Delivery::Pending)
            .count();
        if pending == 0 {
            return Ok(());
        } // Durable decisions also mark already-charged retries.
        if pending != message.attachments.len() {
            return Err(Error::state_inconsistent(
                "Attachment delivery decisions are incomplete.",
            ));
        }
        let retained = self.image_usage(&message.recipient)?;
        let added = images::select(&mut message.attachments, retained);
        if added.images > 0 {
            self.execute_cached(
                queries::PLAN_MESSAGE_IMAGES_INSERT,
                params![
                    message.recipient,
                    crate::convert::capped_i64(retained.bytes + added.bytes),
                    retained.images + added.images
                ],
            )?;
        }
        self.save_attachment_delivery(&message.id, &message.attachments)
    }
    pub(in crate::runtime) fn save_attachment_delivery(
        &self,
        id: &str,
        items: &[crate::runtime::attachments::Attachment],
    ) -> Result<()> {
        for (position, item) in items.iter().enumerate() {
            let data = serde_json::to_string(item).map_err(|e| {
                Error::database_error("Cannot encode attachment delivery.").with_source(e)
            })?;
            let changed = self.execute_cached(
                queries::SAVE_ATTACHMENT_DELIVERY_UPDATE,
                params![
                    id,
                    crate::convert::capped_i64(position as u64),
                    data,
                    item.key()
                ],
            )?;
            if changed != 1 {
                return Err(Error::state_inconsistent(
                    "Attachment delivery lost its message reference.",
                ));
            }
        }
        Ok(())
    }
    pub(in crate::runtime) fn inherit_image_usage(&self, child: &str, source: &str) -> Result<()> {
        self.execute_cached(queries::INHERIT_IMAGE_USAGE_INSERT, params![child, source])?;
        Ok(())
    }
}
