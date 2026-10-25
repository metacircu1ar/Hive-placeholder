use crate::Page;
use crate::values::{LinkKind, Slug};
use pulldown_cmark::{Event, Parser, Tag, TagEnd, TextMergeStream};
use std::collections::BTreeSet;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Link {
    pub target: Slug,
    pub kind: LinkKind,
}

pub(crate) fn links(slug: &Slug, page: &Page) -> BTreeSet<Link> {
    let mut found = BTreeSet::new();
    for text in
        std::iter::once(page.compiled.as_str()).chain(page.timeline.iter().map(|e| e.text.as_str()))
    {
        let mut in_code = false;
        for event in TextMergeStream::new(Parser::new(text)) {
            match event {
                Event::Start(Tag::CodeBlock(_)) => in_code = true,
                Event::End(TagEnd::CodeBlock) => in_code = false,
                Event::Start(Tag::Link { dest_url, .. }) => {
                    if let Some(target) = target(slug, &dest_url) {
                        found.insert(Link {
                            target,
                            kind: LinkKind::Markdown,
                        });
                    }
                }
                Event::Text(value) if !in_code => {
                    for part in value.split("[[").skip(1) {
                        if let Some((link, _)) = part.split_once("]]") {
                            let link = link
                                .split('|')
                                .next()
                                .unwrap_or(link)
                                .split('#')
                                .next()
                                .unwrap_or("");
                            let link = link.to_ascii_lowercase();
                            if let Ok(link) = link.parse::<Slug>() {
                                found.insert(Link {
                                    target: link,
                                    kind: LinkKind::Slug,
                                });
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    found
}
fn target(slug: &Slug, link: &str) -> Option<Slug> {
    let raw = link.split('#').next()?;
    if raw.contains(':') || raw.starts_with('/') || raw.is_empty() {
        return None;
    }
    let normalized = raw.to_ascii_lowercase();
    let raw = normalized.strip_suffix(".md").unwrap_or(&normalized);
    let base = slug.as_str().rsplit_once('/').map_or("", |(base, _)| base);
    let mut pieces: Vec<_> = base.split('/').filter(|s| !s.is_empty()).collect();
    for part in raw.split('/') {
        match part {
            ".." => {
                pieces.pop()?;
            }
            "." => {}
            other => pieces.push(other),
        }
    }
    let result = pieces.join("/");
    result.parse().ok()
}
