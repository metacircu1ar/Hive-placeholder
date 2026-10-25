//! Ordinary-agent consolidation with one recoverable two-page intent.
use super::mutation::{Cas, Change, Required, Target, Ticket};
use super::{Comb, Document, Mutation, Write};
use crate::disk::{ConsolidationIntent, PreparedPage};
use crate::error::CommitExt;
use crate::read::{FileFailure, Publishing};
use crate::{
    Error, Page, Result,
    page::{
        self, ConsolidationWitness, ContentWitness, Decision, Entry, EntryAuthored, EntryMeta,
        ProposalDecision,
    },
    values::{ContentHash, EvidenceId, ReceiptOperation, RequestId, Slug},
};
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

#[derive(Debug, Serialize, Deserialize)]
pub struct Consolidated {
    pub request_id: RequestId,
    pub target: Mutation,
    pub note: Mutation,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CopiedEvidence {
    recopy: bool,
    /// First copy retains the exact authenticated source representation.
    original_markdown: Option<String>,
    /// Later summary changes retain its presentation metadata but not old evidence.
    compiled: Option<Compiled>,
    entries: Vec<Entry>,
}
#[derive(Serialize, Deserialize)]
struct Compiled {
    title: String,
    kind: String,
    tags: Vec<String>,
    aliases: Vec<String>,
    summary: String,
}
impl From<&Page> for Compiled {
    fn from(page: &Page) -> Self {
        Self {
            title: page.metadata.authored.title.clone(),
            kind: page.metadata.authored.kind.clone(),
            tags: page.metadata.authored.tags.clone(),
            aliases: page.metadata.authored.aliases.clone(),
            summary: page.compiled.clone(),
        }
    }
}

#[cfg(test)]
thread_local! {
    static INTERRUPT_AFTER_TARGET: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(crate) fn interrupt_after_target() -> Result<()> {
    if INTERRUPT_AFTER_TARGET.with(|flag| flag.replace(false)) {
        return Err(Error::new(
            crate::ErrorCode::TestInterrupted,
            "Interrupted after target write.",
        ));
    }
    Ok(())
}

impl Comb {
    pub fn consolidate(
        &mut self,
        request: &Consolidate,
        write: &Write<'_>,
    ) -> Result<Consolidated> {
        self.ready()?;
        super::input::consolidation(request, write.run)?;
        let target = Ticket::new(
            write,
            ReceiptOperation::Consolidate,
            (ReceiptOperation::Consolidate, request, write.run),
        )?;
        let prior_target = target.replay(self)?;
        let mark_id = RequestId::derived(crate::error::json_bytes(&(
            ReceiptOperation::ConsolidationSource,
            &write.request_id,
        ))?);
        let mark = write.with_request(&mark_id);
        let source = Ticket::new(
            &mark,
            ReceiptOperation::ConsolidationSource,
            (ReceiptOperation::ConsolidationSource, request, write.run),
        )?;
        // Internal source receipts share the request namespace. Check collisions
        // before either write, including on a new outer request.
        let prior_note = source.replay(self)?;
        self.coordinate_consolidation(request, target, source, prior_target, prior_note)
    }

    fn coordinate_consolidation(
        &mut self,
        request: &Consolidate,
        target: Ticket<'_, '_>,
        source: Ticket<'_, '_>,
        prior_target: Option<Mutation>,
        prior_note: Option<Mutation>,
    ) -> Result<Consolidated> {
        let request_id = target.write().request_id.clone();
        match (prior_target, prior_note) {
            (Some(target), Some(note)) => Ok(Consolidated {
                request_id,
                target,
                note,
            }),
            (None, None) => self.commit_consolidation(request, target, source),
            _ => Err(Error::incomplete_consolidation()),
        }
    }

    fn commit_consolidation(
        &mut self,
        request: &Consolidate,
        target: Ticket<'_, '_>,
        note: Ticket<'_, '_>,
    ) -> Result<Consolidated> {
        let request_id = target.write().request_id.clone();
        let (target_page, note_page) = self.prepare_consolidation(request, target.write())?;
        let pair = ConsolidationIntent {
            target: PreparedPage::new(
                &self.disk,
                &request.target,
                &request.target_hash,
                target_page,
                target.into_receipt(&request.target),
            )?,
            note: PreparedPage::new(
                &self.disk,
                &request.note,
                &request.note_hash,
                note_page,
                note.into_receipt(&request.note),
            )?,
        };
        let publication = self.publication.clone();
        let mut publishing =
            Publishing::file(&mut self.dirty, &publication, FileFailure::RestoreClean);
        publishing.disk_result(self.disk.write_consolidation(&pair))?;
        #[cfg(test)]
        if let Some(hook) = self.before_index.take() {
            hook();
        }
        publishing.publish()?;
        self.index.upsert_consolidation(&pair).committed()?;
        pair.completed(&self.disk).committed()?;
        self.disk.finish()?;
        publishing.commit();
        Ok(Consolidated {
            request_id,
            target: Mutation {
                slug: pair.target.slug,
                hash: pair.target.after,
                request_id: pair.target.receipt.request_id,
                replayed: false,
            },
            note: Mutation {
                slug: pair.note.slug,
                hash: pair.note.after,
                request_id: pair.note.receipt.request_id,
                replayed: false,
            },
        })
    }

    fn prepare_consolidation(
        &mut self,
        request: &Consolidate,
        write: &Write<'_>,
    ) -> Result<(Page, Page)> {
        self.reconcile(&request.note)?;
        self.reconcile(&request.target)?;
        let source = Required(Cas::Matches(Some(&request.note_hash))).load(self, &request.note)?;
        let mut target =
            Required(Cas::Matches(Some(&request.target_hash))).load(self, &request.target)?;
        let evidence = compile(&source, &mut target.page, request, write)?;
        let mut marked = source.page.clone();
        marked.checkpoint(
            &request.target,
            &target.page,
            &evidence,
            write.actor,
            write.run.unwrap_or_default(),
        )?;
        Ok((target.page, marked))
    }

    pub fn decide(
        &mut self,
        slug: &Slug,
        expected: &ContentHash,
        decision: Decision,
        reason: &str,
        write: &super::ReviewedWrite<'_>,
    ) -> Result<Mutation> {
        let write = write.write();
        super::input::decision(slug, reason)?;
        self.mutate(
            write,
            Change {
                slug,
                operation: ReceiptOperation::Decide,
                payload: (
                    ReceiptOperation::Decide,
                    slug,
                    expected,
                    decision,
                    reason,
                    write.run,
                ),
                target: Required(Cas::Matches(Some(expected))),
            },
            || Ok(()),
            |(), document| decide_page(document.page, decision, reason, write),
        )
    }
}

fn decide_page(
    mut page: Page,
    decision: Decision,
    reason: &str,
    write: &Write<'_>,
) -> Result<Page> {
    let record = ProposalDecision {
        decision,
        reason: reason.into(),
        by: write.actor.clone(),
        at: page::now(),
    };
    page.prepend(Entry {
        metadata: metadata("review:explicit-decision".into(), write),
        text: opaque(&record)?,
    });
    page.decide(record);
    Ok(page)
}

fn metadata(source: String, write: &Write<'_>) -> EntryMeta {
    EntryMeta::created_by(
        EntryAuthored {
            id: EvidenceId::generate(),
            source,
        },
        write.actor,
        write.run,
    )
}
/// One JSON string/object inside an indented block cannot introduce Markdown
/// timeline markers or escape via fence-like source content. JSON preserves bytes.
fn opaque(payload: &impl Serialize) -> Result<String> {
    Ok(format!(
        "Source payload (JSON):\n\n    {}",
        crate::error::json_text(payload)?
    ))
}
fn compile(
    source: &Document,
    target: &mut Page,
    request: &Consolidate,
    write: &Write<'_>,
) -> Result<String> {
    let prior = if request.recopy {
        None
    } else {
        coverage(&source.page, target, request)?
    };
    // After a target hand edit only its source reference may survive.
    // This untrusted hint can require inspection, never authorize skipping data.
    let reference = format!("[[{}]] hash:", request.note);
    let lost_copy = target.timeline.iter().any(|entry| {
        entry.metadata.owned().actor() == &page::Actor::External
            && entry.metadata.authored.source.starts_with(&reference)
    });
    if prior.is_none() && !request.recopy && (source.page.metadata.owned().external() || lost_copy)
    {
        return Err(Error::coverage_lost_after_external_edits());
    }
    let current = ContentWitness::new(&source.page)?;
    let (uncopied, changed) = match prior {
        Some((_, content)) => (
            content.uncopied(&source.page)?,
            content.compiled_digest != current.compiled_digest,
        ),
        None => (source.page.timeline.len(), true),
    };
    let prior_id = prior.map(|(id, _)| id.to_owned());
    let evidence = if uncopied == 0 && !changed {
        prior_id.ok_or_else(Error::missing_coverage_evidence)?
    } else {
        let first = prior_id.is_none();
        let copied = CopiedEvidence {
            recopy: request.recopy,
            original_markdown: first.then(|| source.markdown.clone()),
            compiled: (!first && changed).then(|| Compiled::from(&source.page)),
            entries: if first {
                Vec::new()
            } else {
                source.page.timeline[..uncopied].to_vec()
            },
        };
        let meta = metadata(
            format!("[[{}]] hash:{}", request.note, request.note_hash),
            write,
        )
        .with_consolidation(ConsolidationWitness {
            source: request.note.to_string(),
            source_hash: source.hash.to_string(),
            content: current,
            recopy: request.recopy,
        });
        let id = meta.authored.id.clone();
        target.prepend(Entry {
            metadata: meta,
            text: opaque(&copied)?,
        });
        id.to_string()
    };
    target.compiled = request
        .summary
        .trim()
        .replace("[evidence:new]", &format!("[evidence:{evidence}]"));
    target.summary_edited_by(write.actor);
    if write.actor.is_human() {
        target.promote_summary(write.actor.clone())?;
    }
    Ok(evidence)
}

/// Either page can carry authenticated coverage after the other is hand-edited.
/// A source checkpoint is usable only while the referenced target evidence and
/// its complete sealed suffix still exist unchanged.
fn coverage<'a>(
    source: &'a Page,
    target: &'a Page,
    request: &Consolidate,
) -> Result<Option<(&'a str, &'a ContentWitness)>> {
    if let Some(prior) = target.timeline.iter().find_map(|entry| {
        entry
            .metadata
            .owned()
            .consolidation()
            .filter(|w| w.source == request.note.as_str())
            .map(|w| (entry.metadata.authored.id.as_str(), &w.content))
    }) {
        return Ok(Some(prior));
    }
    if let Some(checkpoint) = source
        .metadata
        .owned()
        .consolidated_into()
        .iter()
        .find(|checkpoint| checkpoint.target == request.target.as_str())
    {
        let suffix = checkpoint.target_content.uncopied(target)?;
        if !target.timeline[suffix..]
            .iter()
            .any(|entry| entry.metadata.authored.id.as_str() == checkpoint.target_entry)
        {
            return Err(Error::checkpoint_evidence_lost());
        }
        return Ok(Some((&checkpoint.target_entry, &checkpoint.content)));
    }
    Ok(None)
}

pub use comb_protocol::request::Consolidate;
