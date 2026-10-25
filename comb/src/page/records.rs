//! Raw Markdown records preserve contextual validation and external trust hints.
use super::{
    Actor, Authored, ConsolidationCheckpoint, ConsolidationWitness, Entry, EntryAuthored,
    EntryMeta, EntryOwned, Metadata, Owned, Page, Promotion, ProposalDecision, Receipt, note_type,
};
use crate::values::{EvidenceId, Timestamp};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MetadataRecord {
    title: String,
    #[serde(rename = "type", default = "note_type")]
    kind: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    updated_at: String,
    #[serde(default)]
    author: Option<Actor>,
    #[serde(default)]
    promotion: Option<Promotion>,
    #[serde(default)]
    consolidated_into: Vec<ConsolidationCheckpoint>,
    #[serde(default)]
    decision: Option<ProposalDecision>,
    #[serde(default)]
    external: bool,
    #[serde(default)]
    seal: Option<String>,
}
impl TryFrom<MetadataRecord> for Metadata {
    type Error = Error;
    fn try_from(raw: MetadataRecord) -> Result<Self> {
        let date = |value: String| {
            if value.is_empty() {
                Ok(None)
            } else {
                Timestamp::try_from(value)
                    .map(Some)
                    .map_err(|cause| Error::invalid_page_timestamps().caused_by(cause))
            }
        };
        let created_at = date(raw.created_at)?;
        let updated_at = date(raw.updated_at)?;
        Ok(Self {
            authored: Authored {
                title: raw.title,
                kind: raw.kind,
                tags: raw.tags,
                aliases: raw.aliases,
            },
            owned: Owned {
                created_at,
                updated_at,
                author: raw.author,
                promotion: raw.promotion,
                consolidated_into: raw.consolidated_into,
                decision: raw.decision,
                external: raw.external,
                seal: raw.seal,
            },
        })
    }
}
// These borrowed records pin field order without cloning ownership or allocating maps.
#[derive(Serialize)]
struct MetadataRef<'a> {
    title: &'a str,
    #[serde(rename = "type")]
    kind: &'a str,
    tags: &'a [String],
    aliases: &'a [String],
    created_at: &'a str,
    updated_at: &'a str,
    author: Option<&'a Actor>,
    promotion: Option<&'a Promotion>,
    consolidated_into: &'a [ConsolidationCheckpoint],
    decision: Option<&'a ProposalDecision>,
    external: bool,
    seal: Option<&'a str>,
}
impl<'a> MetadataRef<'a> {
    fn new(metadata: &'a Metadata, seal: Option<&'a str>) -> Self {
        let authored = &metadata.authored;
        let owned = &metadata.owned;
        Self {
            title: &authored.title,
            kind: &authored.kind,
            tags: &authored.tags,
            aliases: &authored.aliases,
            created_at: owned.created_at.as_ref().map_or("", Timestamp::as_str),
            updated_at: owned.updated_at.as_ref().map_or("", Timestamp::as_str),
            author: owned.author.as_ref(),
            promotion: owned.promotion.as_ref(),
            consolidated_into: &owned.consolidated_into,
            decision: owned.decision.as_ref(),
            external: owned.external,
            seal,
        }
    }
}
impl Serialize for Metadata {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        MetadataRef::new(self, self.owned.seal.as_deref()).serialize(serializer)
    }
}
#[derive(Serialize)]
struct UnsignedPage<'a> {
    metadata: MetadataRef<'a>,
    compiled: &'a str,
    timeline: &'a [Entry],
}
pub(super) fn unsigned(page: &Page) -> impl Serialize + '_ {
    UnsignedPage {
        metadata: MetadataRef::new(&page.metadata, None),
        compiled: &page.compiled,
        timeline: &page.timeline,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EntryRecord {
    id: String,
    at: String,
    source: String,
    actor: Actor,
    run: Option<String>,
    #[serde(default)]
    consolidation: Option<ConsolidationWitness>,
}
impl TryFrom<EntryRecord> for EntryMeta {
    type Error = Error;
    fn try_from(raw: EntryRecord) -> Result<Self> {
        // UUID precedes timestamp validation; the caller validates source afterward.
        let id = EvidenceId::try_from(raw.id)?;
        let at = Timestamp::try_from(raw.at)?;
        Ok(Self {
            authored: EntryAuthored {
                id,
                source: raw.source,
            },
            owned: EntryOwned {
                at,
                actor: raw.actor,
                run: raw.run,
                consolidation: raw.consolidation,
            },
        })
    }
}

#[derive(Serialize)]
struct EntryRef<'a> {
    id: &'a EvidenceId,
    at: &'a Timestamp,
    source: &'a str,
    actor: &'a Actor,
    run: Option<&'a str>,
    consolidation: Option<&'a ConsolidationWitness>,
}
impl Serialize for EntryMeta {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        EntryRef {
            id: &self.authored.id,
            at: &self.owned.at,
            source: &self.authored.source,
            actor: &self.owned.actor,
            run: self.owned.run.as_deref(),
            consolidation: self.owned.consolidation.as_ref(),
        }
        .serialize(serializer)
    }
}

/// Receipt claims are decoded before the contextual identity comparison.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReceiptRecord {
    pub principal: String,
    pub request_id: String,
    pub fingerprint: String,
    pub operation: String,
    pub slug: String,
}
impl TryFrom<ReceiptRecord> for Receipt {
    type Error = Error;
    fn try_from(raw: ReceiptRecord) -> Result<Self> {
        Ok(Self {
            principal: raw.principal.try_into()?,
            request_id: raw.request_id.try_into()?,
            fingerprint: raw.fingerprint,
            operation: raw.operation,
            slug: raw.slug.try_into()?,
        })
    }
}
impl From<Receipt> for ReceiptRecord {
    fn from(receipt: Receipt) -> Self {
        Self {
            principal: receipt.principal.into(),
            request_id: receipt.request_id.into(),
            fingerprint: receipt.fingerprint,
            operation: receipt.operation,
            slug: receipt.slug.into(),
        }
    }
}
