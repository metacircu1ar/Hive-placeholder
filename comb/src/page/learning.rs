//! Authenticated consolidation coverage and explicit proposal decisions.
use super::{Actor, Entry, Page, hash};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentWitness {
    pub compiled_digest: String,
    pub timeline_count: usize,
    pub timeline_digest: String,
}
impl ContentWitness {
    pub fn new(page: &Page) -> Result<Self> {
        Ok(Self {
            compiled_digest: page.compiled_digest()?,
            timeline_count: page.timeline.len(),
            timeline_digest: timeline_digest(&page.timeline)?,
        })
    }
    /// Entries prepend to an immutable timeline; the already copied portion is
    /// its suffix. Refuse a damaged witness instead of silently skipping data.
    pub fn uncopied(&self, page: &Page) -> Result<usize> {
        let start = page
            .timeline
            .len()
            .checked_sub(self.timeline_count)
            .ok_or_else(invalid_witness)?;
        if timeline_digest(&page.timeline[start..])? != self.timeline_digest {
            return Err(invalid_witness());
        }
        Ok(start)
    }
    pub fn covers(&self, page: &Page) -> Result<bool> {
        Ok(self.uncopied(page)? == 0 && self.compiled_digest == page.compiled_digest()?)
    }
}
// Coverage describes copied content. External edits strip actor/run/trust metadata;
// that must not invalidate unchanged evidence or restore its previous authorship.
fn timeline_digest(entries: &[Entry]) -> Result<String> {
    Ok(hash(crate::error::json_bytes(
        &entries
            .iter()
            .map(|entry| {
                (
                    &entry.metadata.authored.id,
                    &entry.metadata.owned.at,
                    &entry.metadata.authored.source,
                    &entry.text,
                )
            })
            .collect::<Vec<_>>(),
    )?))
}
fn invalid_witness() -> Error {
    Error::coverage_witness_mismatch()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsolidationWitness {
    pub source: String,
    pub source_hash: String,
    pub content: ContentWitness,
    pub recopy: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsolidationCheckpoint {
    pub target: String,
    pub content: ContentWitness,
    pub target_content: ContentWitness,
    pub target_entry: String,
    pub by: Actor,
    pub run: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalDecision {
    pub decision: Decision,
    pub reason: String,
    pub by: Actor,
    pub at: String,
}

impl Page {
    pub fn inbox_pending(&self) -> Result<bool> {
        for checkpoint in &self.metadata.owned.consolidated_into {
            if checkpoint.content.covers(self)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

impl Page {
    pub(crate) fn checkpoint(
        &mut self,
        target: &crate::values::Slug,
        target_page: &Self,
        evidence: &str,
        actor: &Actor,
        run: &str,
    ) -> Result<()> {
        let content = ContentWitness::new(self)?;
        let checkpoints = &mut self.metadata.owned.consolidated_into;
        checkpoints.retain(|item| item.target != target.as_str());
        if checkpoints.len() >= super::limits::CONSOLIDATION_TARGETS {
            return Err(Error::consolidation_target_limit());
        }
        checkpoints.push(ConsolidationCheckpoint {
            target: target.to_string(),
            content,
            target_content: ContentWitness::new(target_page)?,
            target_entry: evidence.into(),
            by: actor.clone(),
            run: run.into(),
        });
        Ok(())
    }
}

pub use comb_protocol::page::Decision;
