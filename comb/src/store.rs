//! Comb operations: one owned writer, CAS edits, durable receipts and index recovery.
use crate::{
    Actor, Error, Page, Result,
    disk::Disk,
    index::Index,
    page::{self, Authored, Entry, EntryAuthored, EntryMeta, Receipt},
    search::Filter,
    values::{
        CombId, ContentHash, EvidenceId, FindingKind, Fingerprint, Prefix, Principal,
        ReceiptOperation, RequestId, Slug,
    },
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::Path};

mod doctor;
mod document;
mod findings;
pub mod input;
mod learning;
#[cfg(test)]
pub(crate) use learning::interrupt_after_target;
pub(crate) mod limits;
mod mutation;
mod open;
mod operations;
pub(crate) use operations::import_receipt;
mod reconcile;
mod removal;
mod reviewed;
use findings::Findings;
pub use learning::{Consolidate, Consolidated};
pub(crate) use limits::FINDING_LIMIT;
use mutation::check_expected;
pub use removal::Removed;
pub use reviewed::ReviewedWrite;

pub struct Comb {
    pub(crate) index: Index,
    dirty: bool,
    pub(crate) embedding_worker_error: Option<Error>,
    pub(crate) embedding_runtime: Option<crate::embedding::RuntimeInfo>,
    pub(crate) publication: std::sync::Arc<crate::read::Publication>,
    pub(crate) health: std::sync::Arc<crate::locks::Health>,
    #[cfg(test)]
    pub(crate) before_index: Option<Box<dyn FnOnce() + Send>>,
    // SQLite close may checkpoint. Root ownership must outlive every connection
    // and other field, including during unwinding and the last service drop.
    pub(crate) disk: Disk,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Document {
    pub slug: Slug,
    pub hash: ContentHash,
    pub page: Page,
    pub markdown: String,
    pub unverified: bool,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Mutation {
    pub slug: Slug,
    pub hash: ContentHash,
    pub request_id: RequestId,
    pub replayed: bool,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Summary {
    pub slug: Slug,
    pub title: String,
    pub kind: String,
    pub hash: ContentHash,
    pub unverified: bool,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Stats {
    pub embedding_runtime: Option<crate::embedding::RuntimeInfo>,
    pub pages: u64,
    pub chunks: u64,
    pub links: u64,
    pub inbox: u64,
    pub unverified: u64,
    pub unresolved_proposals: u64,
    pub quarantined_pages: u64,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Finding {
    pub slug: String,
    pub kind: FindingKind,
    pub detail: String,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Doctor {
    pub embeddings: crate::embedding::Coverage,
    pub embedding_worker_error: Option<Error>,
    pub stats: Stats,
    pub findings: Vec<Finding>,
    pub findings_truncated: bool,
    pub citation_check: String,
}
#[derive(Debug)]
pub struct Write<'a> {
    actor: &'a Actor,
    principal: Principal,
    request_id: RequestId,
    run: Option<&'a str>,
}
impl<'a> Write<'a> {
    pub fn new(actor: &'a Actor, request_id: &RequestId, run: Option<&'a str>) -> Result<Self> {
        Ok(Self {
            actor,
            principal: Principal::for_actor(actor)?,
            request_id: request_id.clone(),
            run,
        })
    }
    pub(crate) fn authorized(
        authority: &'a crate::service::auth::Authority,
        request_id: &RequestId,
        run: Option<&'a str>,
    ) -> Self {
        Self {
            actor: authority.actor(),
            principal: authority.principal().clone(),
            request_id: request_id.clone(),
            run,
        }
    }
    fn with_request<'b>(&'b self, request_id: &RequestId) -> Write<'b> {
        Write {
            actor: self.actor,
            principal: self.principal.clone(),
            request_id: request_id.clone(),
            run: self.run,
        }
    }
}

impl Comb {
    /// Checked, immutable access to derived reads. Mutations remain on Comb.
    pub fn index(&self) -> Result<&Index> {
        self.ready()?;
        Ok(&self.index)
    }
    pub fn identity(&self) -> CombId {
        self.disk.id()
    }
    pub(crate) fn ready(&self) -> Result<()> {
        self.health.check()?;
        if self.dirty {
            Err(Error::pending_index())
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod ownership_tests;
#[cfg(test)]
mod poison_tests;
#[cfg(test)]
mod publication_tests;
#[cfg(test)]
mod removal_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod edit_tests;
#[cfg(test)]
mod loading_tests;
#[cfg(test)]
mod pipeline_tests;

#[cfg(test)]
mod scan_tests;

pub use comb_protocol::request::List;
