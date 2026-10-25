//! Store input, import and diagnostic presentation policies.
pub(crate) const FINDING_LIMIT: u16 = 1024;
pub const EVIDENCE_TEXT_BYTES: usize = crate::page::PAGE_BYTES / 2;
pub(super) const CAPTURE_TITLE_CHARS: usize = 120;
pub const DECISION_REASON_BYTES: usize = 4096;
pub(super) const FINDING_DETAIL_CHARS: usize = 512;
pub(super) const PAGE_NEAR_LIMIT_BYTES: usize = crate::page::PAGE_BYTES * 3 / 4;
pub(super) const IGNORE_BYTES: usize = 1024;
pub(super) const CITATION_CHARS: usize = 160;
pub(crate) const IMPORT_BYTES: usize = 16 * 1024 * 1024;
pub(crate) use comb_protocol::limits::IMPORT_PAGES;
