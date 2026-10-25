//! Ownership transitions. Deserialization alone never authenticates these claims.
use super::{
    Actor, Authored, ConsolidationWitness, Entry, EntryAuthored, EntryMeta, EntryOwned, Metadata,
    Owned, Page, Promotion, ProposalDecision, ReviewedSuffix, Timestamp, hash, now,
};
use crate::Result;

impl Metadata {
    pub fn owned(&self) -> &Owned {
        &self.owned
    }
}
impl Owned {
    pub fn created_at(&self) -> Option<&Timestamp> {
        self.created_at.as_ref()
    }
    pub fn updated_at(&self) -> Option<&Timestamp> {
        self.updated_at.as_ref()
    }
    pub fn author(&self) -> Option<&Actor> {
        self.author.as_ref()
    }
    pub fn promotion(&self) -> Option<&Promotion> {
        self.promotion.as_ref()
    }
    pub fn consolidated_into(&self) -> &[super::ConsolidationCheckpoint] {
        &self.consolidated_into
    }
    pub fn decision(&self) -> Option<&ProposalDecision> {
        self.decision.as_ref()
    }
    pub fn external(&self) -> bool {
        self.external
    }
    pub fn seal(&self) -> Option<&str> {
        self.seal.as_deref()
    }
    fn created_by(actor: &Actor) -> Self {
        Self {
            created_at: Some(Timestamp::now()),
            updated_at: None,
            author: Some(actor.clone()),
            promotion: None,
            consolidated_into: Vec::new(),
            decision: None,
            external: false,
            seal: None,
        }
    }
    fn edited_by(mut self, actor: &Actor) -> Self {
        self.author = Some(actor.clone());
        self.external = false;
        self.decision = None;
        self
    }
}
impl EntryMeta {
    pub fn owned(&self) -> &EntryOwned {
        &self.owned
    }
    pub(crate) fn created_by(authored: EntryAuthored, actor: &Actor, run: Option<&str>) -> Self {
        Self {
            authored,
            owned: EntryOwned {
                at: Timestamp::now(),
                actor: actor.clone(),
                run: run.map(str::to_owned),
                consolidation: None,
            },
        }
    }
    pub(crate) fn with_consolidation(mut self, witness: ConsolidationWitness) -> Self {
        self.owned.consolidation = Some(witness);
        self
    }
}
impl EntryOwned {
    pub fn at(&self) -> &Timestamp {
        &self.at
    }
    pub fn actor(&self) -> &Actor {
        &self.actor
    }
    pub fn run(&self) -> Option<&str> {
        self.run.as_deref()
    }
    pub fn consolidation(&self) -> Option<&ConsolidationWitness> {
        self.consolidation.as_ref()
    }
    fn externalize(&mut self) {
        self.actor = Actor::External;
        self.run = None;
        self.consolidation = None;
    }
}
impl Page {
    pub(crate) fn from_put(
        authored: Authored,
        compiled: String,
        mut additions: Vec<Entry>,
        existing: Option<Self>,
        actor: &Actor,
    ) -> Self {
        let owned = match existing {
            Some(existing) => {
                additions.extend(existing.timeline);
                existing.metadata.owned.edited_by(actor)
            }
            None => Owned::created_by(actor),
        };
        Self {
            metadata: Metadata { authored, owned },
            compiled,
            timeline: additions,
        }
    }
    pub(crate) fn prepend(&mut self, entry: Entry) {
        self.timeline.insert(0, entry);
        self.metadata.owned.decision = None;
    }
    pub(crate) fn summary_edited_by(&mut self, actor: &Actor) {
        self.metadata.owned.author = Some(actor.clone());
        self.metadata.owned.decision = None;
    }
    pub(crate) fn touch(&mut self) {
        self.metadata.owned.updated_at = Some(Timestamp::now());
    }
    pub(crate) fn install_seal(&mut self, seal: String) {
        self.metadata.owned.seal = Some(seal);
    }
    pub(crate) fn unsigned(&self) -> impl serde::Serialize + '_ {
        super::records::unsigned(self)
    }
    pub(crate) fn decide(&mut self, decision: ProposalDecision) {
        self.metadata.owned.decision = Some(decision);
    }
    pub(crate) fn promote_summary(&mut self, actor: Actor) -> Result<()> {
        let evidence = self
            .metadata
            .owned
            .promotion
            .as_ref()
            .and_then(|p| p.evidence.clone());
        self.metadata.owned.external = false;
        self.metadata.owned.promotion = Some(Promotion {
            digest: self.compiled_digest()?,
            by: actor,
            at: now(),
            evidence,
        });
        Ok(())
    }
    pub(crate) fn promote(&mut self, actor: Actor) -> Result<()> {
        let evidence = Some(ReviewedSuffix {
            count: self.timeline.len(),
            digest: hash(crate::error::json_bytes(&self.timeline)?),
            by: actor.clone(),
            at: now(),
        });
        self.metadata.owned.external = false;
        self.metadata.owned.promotion = Some(Promotion {
            digest: self.compiled_digest()?,
            by: actor,
            at: now(),
            evidence,
        });
        Ok(())
    }
    pub(crate) fn externalize(&mut self) {
        let owned = &mut self.metadata.owned;
        owned.author = Some(Actor::External);
        owned.external = true;
        owned.promotion = None;
        owned.consolidated_into.clear();
        owned.decision = None;
        owned.seal = None;
        for entry in &mut self.timeline {
            entry.metadata.owned.externalize();
        }
    }
}
