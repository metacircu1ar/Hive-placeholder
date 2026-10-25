use super::limits::CAPTURE_TITLE_CHARS;
use super::mutation::{Cas, Change, Optional, Required, Target, Ticket};
use super::{
    Actor, Authored, Comb, ContentHash, Document, Entry, EntryAuthored, EntryMeta, Error,
    EvidenceId, Mutation, Page, Prefix, ReceiptOperation, Result, ReviewedWrite, Slug, Write, page,
};

// Only caller-authored state crosses into the put transformation. The retained
// suffix and its ownership come separately from the current loaded page.
struct PutDraft {
    authored: Authored,
    compiled: String,
    additions: Vec<(EntryAuthored, String)>,
}

impl PutDraft {
    fn extract(mut page: Page, existing: Option<&Document>) -> Result<Self> {
        let old_len = existing.map_or(0, |d| d.page.timeline.len());
        if let Some(old) = existing
            && !page.timeline.ends_with(&old.page.timeline)
        {
            return Err(Error::immutable_timeline());
        }
        let additions = page.timeline.len() - old_len;
        page.timeline.truncate(additions);
        Ok(Self {
            authored: page.metadata.authored,
            compiled: page.compiled,
            additions: page
                .timeline
                .into_iter()
                .map(|entry| (entry.metadata.authored, entry.text))
                .collect(),
        })
    }

    fn apply(self, existing: Option<Page>, write: &Write<'_>) -> Result<Page> {
        let additions = self
            .additions
            .into_iter()
            .map(|(authored, text)| Entry {
                metadata: EntryMeta::created_by(authored, write.actor, write.run),
                text,
            })
            .collect();
        let mut page = Page::from_put(
            self.authored,
            self.compiled,
            additions,
            existing,
            write.actor,
        );
        if write.actor.is_human() {
            page.promote_summary(write.actor.clone())?;
        }
        Ok(page)
    }
}

impl Comb {
    pub fn put_markdown(
        &mut self,
        slug: &Slug,
        markdown: &str,
        expected: Option<&ContentHash>,
        write: &Write<'_>,
    ) -> Result<Mutation> {
        self.put_authored(
            slug,
            markdown,
            expected,
            write,
            comb_protocol::service::input::mutation::Write::Put,
        )
    }

    fn put_authored(
        &mut self,
        slug: &Slug,
        markdown: &str,
        expected: Option<&ContentHash>,
        write: &Write<'_>,
        kind: comb_protocol::service::input::mutation::Write,
    ) -> Result<Mutation> {
        self.mutate(
            write,
            Change {
                slug,
                operation: ReceiptOperation::Put,
                payload: (ReceiptOperation::Put, slug, markdown, expected, write.run),
                target: Optional(expected),
            },
            || super::input::markdown(kind, markdown),
            |page, existing| prepare_put(page, existing, write),
        )
    }

    pub(crate) fn preflight_import_page(
        &self,
        slug: &Slug,
        markdown: &str,
        expected: Option<&ContentHash>,
        request_id: &crate::values::RequestId,
    ) -> Result<()> {
        let write = Write::new(&Actor::Human, request_id, None)?;
        let page = Page::parse(markdown)?;
        let mut page = prepare_put(page, Optional(expected).load(self, slug)?, &write)?;
        page.touch();
        self.disk.seal(slug, &mut page)?;
        let raw = page.render()?;
        if crate::error::value(Page::parse(&raw)?)? != crate::error::value(page)? {
            return Err(Error::page_roundtrip_failed());
        }
        Ok(())
    }

    pub fn append(
        &mut self,
        slug: &Slug,
        text: &str,
        source: &str,
        compiled: Option<&str>,
        expected: Option<&ContentHash>,
        write: &Write<'_>,
    ) -> Result<Mutation> {
        super::input::evidence(text, source)?;
        self.mutate(
            write,
            Change {
                slug,
                operation: ReceiptOperation::Append,
                payload: (
                    ReceiptOperation::Append,
                    slug,
                    text,
                    source,
                    compiled,
                    expected,
                    write.run,
                ),
                target: Required(if compiled.is_some() || expected.is_some() {
                    Cas::Matches(expected)
                } else {
                    Cas::Any
                }),
            },
            || super::input::summary_hash(compiled, expected),
            |(), document| append_page(document.page, text, source, compiled, write),
        )
    }

    pub fn promote(
        &mut self,
        slug: &Slug,
        expected: &ContentHash,
        write: &ReviewedWrite<'_>,
    ) -> Result<Mutation> {
        let write = write.write();
        self.mutate(
            write,
            Change {
                slug,
                operation: ReceiptOperation::Promote,
                payload: (ReceiptOperation::Promote, slug, expected),
                target: Required(Cas::Matches(Some(expected))),
            },
            || Ok(()),
            |(), mut document| {
                document.page.promote(write.actor.clone())?;
                Ok(document.page)
            },
        )
    }

    pub fn capture(&mut self, text: &str, write: &Write<'_>) -> Result<Mutation> {
        super::input::capture(text)?;
        let slug = Prefix::capture_slug(&page::hash(crate::error::json_bytes(&(
            &write.principal,
            &write.request_id,
        ))?))?;
        let title = capture_title(text);
        let markdown = Page::note(&title, text)?;
        self.put_authored(
            &slug,
            &markdown,
            None,
            write,
            comb_protocol::service::input::mutation::Write::Capture,
        )
    }
}

fn prepare_put(page: Page, existing: Option<Document>, write: &Write<'_>) -> Result<Page> {
    PutDraft::extract(page, existing.as_ref())?.apply(existing.map(|document| document.page), write)
}

pub(crate) fn import_receipt(
    slug: &Slug,
    markdown: &str,
    expected: Option<&ContentHash>,
    request_id: &crate::values::RequestId,
) -> Result<crate::page::Receipt> {
    let write = Write::new(&Actor::Human, request_id, None)?;
    Ok(Ticket::new(
        &write,
        ReceiptOperation::Put,
        (ReceiptOperation::Put, slug, markdown, expected, write.run),
    )?
    .into_receipt(slug))
}

fn append_page(
    mut page: Page,
    text: &str,
    source: &str,
    compiled: Option<&str>,
    write: &Write<'_>,
) -> Result<Page> {
    let evidence_id = EvidenceId::generate();
    page.prepend(Entry {
        metadata: EntryMeta::created_by(
            EntryAuthored {
                id: evidence_id.clone(),
                source: source.into(),
            },
            write.actor,
            write.run,
        ),
        text: text.trim().into(),
    });
    if let Some(compiled) = compiled {
        page.compiled = compiled
            .trim()
            .replace("[evidence:new]", &format!("[evidence:{evidence_id}]"));
        page.summary_edited_by(write.actor);
        if write.actor.is_human() {
            page.promote_summary(write.actor.clone())?;
        }
    }
    Ok(page)
}

fn capture_title(text: &str) -> String {
    let title = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("Note")
        .chars()
        .take(CAPTURE_TITLE_CHARS)
        .collect::<String>();
    if title.trim().is_empty() {
        "Note".into()
    } else {
        title
    }
}
