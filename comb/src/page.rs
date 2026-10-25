//! Strict Markdown pages, evidence provenance and content-based review state.
use crate::{
    Error, Result,
    values::{EvidenceId, Timestamp},
};
pub use comb_protocol::page::{Actor, hash};
use pulldown_cmark::{Event, Parser, Tag};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

mod learning;
pub(crate) mod limits;
mod ownership;
mod records;
pub use learning::{
    ConsolidationCheckpoint, ConsolidationWitness, ContentWitness, Decision, ProposalDecision,
};
pub(crate) use records::ReceiptRecord;
use records::{EntryRecord, MetadataRecord};

pub use limits::PAGE_BYTES;
use limits::{ALIAS_COUNT, KIND_BYTES, LABEL_BYTES, SOURCE_BYTES, TAG_COUNT, TITLE_BYTES};
const ENTRY: &str = "<!-- comb-entry ";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Promotion {
    pub digest: String,
    pub by: Actor,
    pub at: String,
    pub evidence: Option<ReviewedSuffix>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewedSuffix {
    pub count: usize,
    pub digest: String,
    pub by: Actor,
    pub at: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ReceiptRecord")]
pub struct Receipt {
    pub principal: crate::values::Principal,
    pub request_id: crate::values::RequestId,
    pub fingerprint: String,
    pub operation: String,
    pub slug: crate::values::Slug,
}
#[derive(Clone, Debug)]
pub struct Authored {
    pub title: String,
    pub kind: String,
    pub tags: Vec<String>,
    pub aliases: Vec<String>,
}
/// Parsed ownership remains an untrusted claim until the head and seal agree.
#[derive(Clone, Debug)]
pub struct Owned {
    created_at: Option<Timestamp>,
    updated_at: Option<Timestamp>,
    author: Option<Actor>,
    promotion: Option<Promotion>,
    consolidated_into: Vec<ConsolidationCheckpoint>,
    decision: Option<ProposalDecision>,
    external: bool,
    seal: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(try_from = "MetadataRecord")]
pub struct Metadata {
    pub authored: Authored,
    owned: Owned,
}
fn note_type() -> String {
    "note".into()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryAuthored {
    pub id: EvidenceId,
    pub source: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryOwned {
    at: Timestamp,
    actor: Actor,
    run: Option<String>,
    consolidation: Option<ConsolidationWitness>,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "EntryRecord")]
pub struct EntryMeta {
    pub authored: EntryAuthored,
    owned: EntryOwned,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub metadata: EntryMeta,
    pub text: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub metadata: Metadata,
    pub compiled: String,
    pub timeline: Vec<Entry>,
}

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

impl Page {
    /// Original note input bytes, before parsing or ownership rendering.
    pub(crate) fn note(title: &str, body: &str) -> Result<String> {
        Ok(format!(
            "---\ntitle: {}\ntype: note\n---\n{}",
            crate::error::json_text(title)?,
            body
        ))
    }

    pub fn parse(markdown: &str) -> Result<Self> {
        if markdown.len() > PAGE_BYTES || markdown.contains('\0') {
            return Err(Error::invalid_page_content());
        }
        let rest = markdown
            .strip_prefix("---\n")
            .ok_or_else(Error::frontmatter_missing)?;
        let (yaml, body) = rest
            .split_once("\n---\n")
            .ok_or_else(Error::unclosed_frontmatter)?;
        let metadata = Metadata::try_from(serde_yaml_ng::from_str::<MetadataRecord>(yaml)?)?;
        if metadata.authored.title.trim().is_empty()
            || metadata.authored.title.len() > TITLE_BYTES
            || metadata.authored.kind.is_empty()
            || metadata.authored.kind.len() > KIND_BYTES
            || metadata.authored.tags.len() > TAG_COUNT
            || metadata.authored.aliases.len() > ALIAS_COUNT
            || metadata
                .authored
                .tags
                .iter()
                .chain(&metadata.authored.aliases)
                .any(|s| s.trim().is_empty() || s.len() > LABEL_BYTES)
        {
            return Err(Error::invalid_page_metadata());
        }
        let mut depth = 0;
        let mut split = None;
        for (event, range) in Parser::new(body).into_offset_iter() {
            match event {
                Event::Start(_) => depth += 1,
                Event::End(_) => depth -= 1,
                Event::Rule if depth == 0 => {
                    let tail = body[range.end..].trim_start_matches('\n');
                    if let Some(timeline) = tail.strip_prefix("## Timeline\n") {
                        split = Some((body[..range.start].trim_end().to_owned(), timeline));
                        break;
                    }
                }
                _ => {}
            }
        }
        let (compiled, timeline) = split.unwrap_or((body.trim().to_owned(), ""));
        let timeline = parse_timeline(timeline)?;
        Ok(Self {
            metadata,
            compiled,
            timeline,
        })
    }
    pub fn render(&self) -> Result<String> {
        let yaml = crate::error::yaml_text(&self.metadata)?;
        let mut out = format!("---\n{yaml}---\n{}\n\n---\n## Timeline\n", self.compiled);
        for entry in &self.timeline {
            let metadata = crate::error::json_text(&entry.metadata)?
                .replace('<', "\\u003c")
                .replace('>', "\\u003e");
            out.push('\n');
            out.push_str(ENTRY);
            out.push_str(&metadata);
            out.push_str(" -->\n");
            out.push_str(&entry.text);
            out.push('\n');
        }
        if out.len() > PAGE_BYTES {
            return Err(Error::page_limit());
        }
        Ok(out)
    }
    pub fn compiled_digest(&self) -> Result<String> {
        Ok(hash(crate::error::json_bytes(&(
            &self.metadata.authored.title,
            &self.metadata.authored.kind,
            &self.metadata.authored.tags,
            &self.metadata.authored.aliases,
            &self.compiled,
        ))?))
    }
    pub fn verified(&self) -> bool {
        !self.metadata.owned.external
            && self.metadata.owned.promotion.as_ref().is_some_and(|p| {
                match self.compiled_digest() {
                    Ok(digest) => digest == p.digest,
                    Err(error) => {
                        tracing::warn!(name: "comb.trust_check_failed",
                             error = &error as &(dyn std::error::Error + 'static),
                             decision = "unverified");
                        false
                    }
                }
            })
    }
    /// Validate the reviewed suffix once before chunking; new prepended entries
    /// remain unreviewed unless their authenticated author is human.
    pub fn reviewed_start(&self) -> Option<usize> {
        if self.metadata.owned.external {
            return None;
        }
        let evidence = self.metadata.owned.promotion.as_ref()?.evidence.as_ref()?;
        let start = self.timeline.len().checked_sub(evidence.count)?;
        let encoded = match crate::error::json_bytes(&self.timeline[start..]) {
            Ok(encoded) => encoded,
            Err(error) => {
                tracing::warn!(name: "comb.trust_check_failed",
                     error = &error as &(dyn std::error::Error + 'static),
                     decision = "unreviewed");
                return None;
            }
        };
        let digest = hash(encoded);
        (digest == evidence.digest).then_some(start)
    }
}

fn parse_timeline(text: &str) -> Result<Vec<Entry>> {
    let mut starts = Vec::new();
    let mut depth = 0;
    let mut top_level_html = false;
    for (event, range) in Parser::new(text).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                top_level_html = depth == 0 && matches!(tag, Tag::HtmlBlock);
                depth += 1;
            }
            Event::End(_) => {
                depth -= 1;
                top_level_html = false;
            }
            Event::Html(raw) if top_level_html && raw.starts_with(ENTRY) => {
                let json = raw
                    .trim()
                    .strip_prefix(ENTRY)
                    .and_then(|s| s.strip_suffix(" -->"))
                    .ok_or_else(Error::invalid_evidence_marker)?;
                let meta = EntryMeta::try_from(serde_json::from_str::<EntryRecord>(json)?)?;
                if meta.authored.source.trim().is_empty()
                    || meta.authored.source.len() > SOURCE_BYTES
                {
                    return Err(Error::invalid_evidence_source());
                }
                starts.push((range, meta));
            }
            _ => {}
        }
    }
    let leading = starts.first().map_or(text, |(r, _)| &text[..r.start]);
    if !leading.trim().is_empty() {
        return Err(Error::unstructured_timeline());
    }
    let mut ids = BTreeSet::new();
    starts
        .iter()
        .enumerate()
        .map(|(i, (range, meta))| {
            if !ids.insert(&meta.authored.id) {
                return Err(Error::duplicate_evidence_id());
            }
            let end = starts.get(i + 1).map_or(text.len(), |(r, _)| r.start);
            Ok(Entry {
                metadata: meta.clone(),
                text: text[range.end..end].trim().to_owned(),
            })
        })
        .collect()
}
