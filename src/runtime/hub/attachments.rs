//! Publication and deletion share one gate retained through actual writer COMMIT.
use super::Hub;
use crate::error::{Error, Result};
use std::sync::Arc;

impl Hub {
    pub(in crate::runtime) async fn settle_publication(
        &self,
        publication: crate::runtime::attachments::Publication,
        result: Result<Option<serde_json::Value>>,
    ) -> Result<Option<serde_json::Value>> {
        if let Err(secondary) = self.cleanup_publication(publication).await {
            if let Err(primary) = &result {
                tracing::warn!(name: "attachments.publication_cleanup_failed",
                     primary = primary as &(dyn std::error::Error + 'static),
                     secondary = &secondary as &(dyn std::error::Error + 'static),
                     decision = "preserve_request_error");
            } else {
                tracing::warn!(name: "attachments.publication_cleanup_failed",
                     error = &secondary as &(dyn std::error::Error + 'static),
                     decision = "retain_for_recovery");
            }
            // A stale preparation has no accepted outcome to preserve. Stop its
            // retry; recovery will inspect the retained unindexed publication.
            if matches!(result, Ok(None)) {
                return Err(secondary);
            }
        }
        result
    }

    pub(in crate::runtime) async fn cleanup_publication(
        &self,
        publication: crate::runtime::attachments::Publication,
    ) -> Result<()> {
        let crate::runtime::attachments::Publication { root, files, guard } = publication;
        if files.is_empty() {
            return Ok(());
        }
        let (unreferenced, owner) = self
            .with(move |core| {
                Ok((
                    core.store().unindexed_attachments(files)?,
                    core.daemon_lock().clone(),
                ))
            })
            .await?;
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let _owner = owner;
            for file in unreferenced {
                crate::runtime::attachments::remove_snapshot(&root, &file.key, &file.filename)?;
            }
            Ok::<_, Error>(())
        })
        .await
        .map_err(|e| {
            Error::state_io("Attachment rollback cleanup did not finish.").with_source(e)
        })?
    }

    pub(in crate::runtime) async fn cleanup_attachments(&self) -> Result<bool> {
        let gate = Arc::new(self.attachment_gate.clone().lock_owned().await);
        // Fetch only after taking the gate: an older outbox row can have been reused.
        let (root, pending, owner) = self
            .with(|core| {
                let (root, pending) = core.store().pending_attachments()?;
                Ok((root, pending, core.daemon_lock().clone()))
            })
            .await?;
        if pending.is_empty() {
            return Ok(false);
        }
        let blocking_gate = gate.clone();
        let removed = tokio::task::spawn_blocking(move || {
            let _owner = owner;
            let _gate = blocking_gate;
            for item in &pending {
                crate::runtime::attachments::remove_snapshot(&root, &item.key, &item.filename)?;
            }
            Ok::<_, Error>(pending)
        })
        .await
        .map_err(|e| Error::state_io("Attachment cleanup did not finish.").with_source(e))??;
        let count = removed.len();
        self.with_retained(gate, move |core| {
            core.store().acknowledge_attachments(&removed)?;
            Ok(!core.store().pending_attachments()?.1.is_empty())
        })
        .await
        .inspect(|_| {
            tracing::info!(name: "attachments.cleanup", count = count);
        })
    }
}
