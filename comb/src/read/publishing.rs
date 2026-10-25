//! File publication owns its failure fence; derived work only owns the short gate.
use super::{Publication, PublicationState};
use crate::error::CommitExt;
use crate::{ErrorCode, Result, locks::Guard};
use std::sync::MutexGuard;

pub(crate) enum FileFailure {
    RestoreClean,
    Fence,
}

pub(crate) struct Publishing<'a> {
    gate: Guard<'a, MutexGuard<'a, PublicationState>>,
}

impl<'a> Publishing<'a> {
    /// Prepare a file mutation without extending the publication critical section.
    pub(crate) fn file(
        dirty: &'a mut bool,
        publication: &'a Publication,
        failure: FileFailure,
    ) -> FilePublishing<'a> {
        *dirty = true;
        FilePublishing {
            dirty,
            publication,
            failure,
            pending: true,
            publishing: None,
        }
    }

    /// SQL rollback and ordinary post-COMMIT cache errors do not dirty Comb.
    pub(crate) fn derived(publication: &'a Publication) -> Result<Self> {
        Ok(Self {
            gate: publication.lock()?,
        })
    }
}

/// Keeps the dirty borrow separate from Disk/Index and the Arc owning publication.
/// Only file publications expose disk-result, publish and commit transitions.
pub(crate) struct FilePublishing<'a> {
    dirty: &'a mut bool,
    publication: &'a Publication,
    failure: FileFailure,
    pending: bool,
    publishing: Option<Publishing<'a>>,
}

impl FilePublishing<'_> {
    pub(crate) fn disk_result(&mut self, result: Result<()>) -> Result<()> {
        if let Err(error) = &result
            // Removal retains its conservative I/O fence, but a proven conflict
            // before intent acceptance has changed no authoritative state.
            && (matches!(self.failure, FileFailure::RestoreClean)
                || error.code == ErrorCode::Conflict)
            && !error.file_committed
            && error.code != ErrorCode::RecoveryRequired
        {
            *self.dirty = false;
            self.pending = false;
        }
        result
    }

    pub(crate) fn publish(&mut self) -> Result<()> {
        let mut publishing = Publishing::derived(self.publication).committed()?;
        *publishing.gate = PublicationState::Pending;
        self.publishing = Some(publishing);
        Ok(())
    }

    pub(crate) fn commit(&mut self) {
        *self.dirty = false;
        self.pending = false;
        if let Some(publishing) = &mut self.publishing {
            *publishing.gate = PublicationState::Ready;
        }
        // Retain the checked guard until the existing lexical scope ends.
    }
}

impl Drop for FilePublishing<'_> {
    fn drop(&mut self) {
        if self.pending {
            if let Some(publishing) = &mut self.publishing {
                *publishing.gate = PublicationState::Pending;
            } else {
                // Never reacquire while holding publication. A permanent Health
                // failure may reject this acquisition; it already fences readers.
                self.publication.fail();
            }
        }
    }
}
