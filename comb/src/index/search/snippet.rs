//! Pure Unicode snippet selection; highlights are byte offsets in the result.
use crate::index::limits::{SNIPPET_CHARS, SNIPPET_CONTEXT_CHARS};

pub(super) struct Snippet {
    pub(super) text: String,
    pub(super) highlights: Vec<[usize; 2]>,
}

pub(super) fn snippet(text: &str, query: &str) -> Snippet {
    let terms: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
    let offsets: Vec<_> = text
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(text.len()))
        .collect();
    let mut ranges = Vec::new();
    for term in terms {
        let length = term.chars().count();
        for window in offsets.windows(length + 1) {
            let (start, end) = (window[0], window[length]);
            if text[start..end].to_lowercase() == term {
                ranges.push([start, end]);
            }
        }
    }
    ranges.sort_unstable();
    ranges.dedup();
    let first = ranges
        .first()
        .map_or(0, |r| offsets.partition_point(|i| *i < r[0]));
    let start = first.saturating_sub(SNIPPET_CONTEXT_CHARS);
    let end = (start + SNIPPET_CHARS).min(offsets.len() - 1);
    let (left, right) = (offsets[start], offsets[end]);
    let highlights = ranges
        .into_iter()
        .filter(|r| r[0] >= left && r[1] <= right)
        .map(|r| [r[0] - left, r[1] - left])
        .collect();
    Snippet {
        text: text[left..right].to_owned(),
        highlights,
    }
}

#[cfg(test)]
mod tests;
