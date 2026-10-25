//! Word-count policies for deterministic chunk boundaries.
pub(super) const MAX_WORDS: usize = 300;
pub(super) const PARAGRAPH_FLOOR: usize = 200;
pub(super) const SENTENCE_FLOOR: usize = 250;
pub(super) const OVERLAP_WORDS: usize = 50;
