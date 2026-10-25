//! Deterministic bounded chunks and Markdown link extraction.
use crate::Page;
use crate::values::Zone;
use pulldown_cmark::{Event, HeadingLevel, Parser, Tag};

mod limits;
mod links;
use limits::{MAX_WORDS, OVERLAP_WORDS, PARAGRAPH_FLOOR, SENTENCE_FLOOR};
pub(crate) use links::{Link, links};

#[cfg(test)]
mod tests;

pub(crate) struct Chunk {
    pub zone: Zone,
    pub heading: String,
    pub text: String,
    pub verified: bool,
}
pub(crate) fn chunks(page: &Page) -> Vec<Chunk> {
    let mut chunks = split(&page.compiled, Zone::Compiled, page.verified());
    let reviewed = page.reviewed_start();
    for (position, entry) in page.timeline.iter().enumerate() {
        chunks.extend(split(
            &entry.text,
            Zone::Timeline,
            entry.metadata.owned().actor().is_human()
                || reviewed.is_some_and(|start| position >= start),
        ));
    }
    chunks
}
fn split(text: &str, zone: Zone, verified: bool) -> Vec<Chunk> {
    let mut result = Vec::new();
    let mut heading = Vec::<(HeadingLevel, String)>::new();
    let mut start = 0;
    for (event, range) in Parser::new(text).into_offset_iter() {
        if let Event::Start(Tag::Heading { level, .. }) = event {
            result.extend(Chunker::new(
                &text[start..range.start],
                &heading
                    .iter()
                    .map(|(_, text)| text.as_str())
                    .collect::<Vec<_>>()
                    .join(" / "),
                zone,
                verified,
            ));
            let line = text[range.start..range.end].lines().next().unwrap_or("");
            while heading.last().is_some_and(|(prior, _)| *prior >= level) {
                heading.pop();
            }
            heading.push((level, line.trim_start_matches('#').trim().to_owned()));
            start = range.end;
        }
    }
    result.extend(Chunker::new(
        &text[start..],
        &heading
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>()
            .join(" / "),
        zone,
        verified,
    ));
    result
}

struct Chunker<'a> {
    words: Vec<&'a str>,
    paragraphs: Vec<usize>,
    heading: &'a str,
    zone: Zone,
    verified: bool,
    start: usize,
}
impl<'a> Chunker<'a> {
    fn new(text: &'a str, heading: &'a str, zone: Zone, verified: bool) -> Self {
        Self {
            words: text.split_whitespace().collect(),
            paragraphs: text
                .split("\n\n")
                .scan(0, |count, paragraph| {
                    *count += paragraph.split_whitespace().count();
                    Some(*count)
                })
                .collect(),
            heading,
            zone,
            verified,
            start: 0,
        }
    }
    fn end(&self) -> usize {
        let end = (self.start + MAX_WORDS).min(self.words.len());
        if end == self.words.len() {
            return end;
        }
        self.paragraphs
            .iter()
            .rev()
            .copied()
            .find(|i| *i >= self.start + PARAGRAPH_FLOOR && *i <= end)
            .or_else(|| {
                (self.start + SENTENCE_FLOOR..end)
                    .rev()
                    .find(|i| self.words[*i].ends_with(['.', '!', '?']))
                    .map(|i| i + 1)
            })
            .unwrap_or(end)
    }
}
impl Iterator for Chunker<'_> {
    type Item = Chunk;

    fn next(&mut self) -> Option<Self::Item> {
        if self.start == self.words.len() {
            return None;
        }
        let end = self.end();
        let chunk = Chunk {
            zone: self.zone,
            heading: self.heading.into(),
            text: self.words[self.start..end].join(" "),
            verified: self.verified,
        };
        self.start = if end == self.words.len() {
            end
        } else {
            end - OVERLAP_WORDS
        };
        Some(chunk)
    }
}
