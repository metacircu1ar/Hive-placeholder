//! Pure authored-text rules shared by CLI planning and store operations.
pub use super::limits::{DECISION_REASON_BYTES, EVIDENCE_TEXT_BYTES};
pub use crate::page::limits::SOURCE_BYTES;
use crate::{
    Error, Page, Result,
    values::{Prefix, Slug},
};
use comb_protocol::service::input::mutation::Write;
use hive_input::Code;

#[cfg(test)]
mod tests;

pub fn nonblank(text: &str, max_bytes: usize) -> bool {
    !text.trim().is_empty() && text.len() <= max_bytes
}

fn rejected(kind: Write, field: &str, cause: Error) -> Error {
    kind.error(Code::InvalidValue, field, false)
        .caused_by(cause)
}
pub(super) fn markdown(kind: Write, markdown: &str) -> Result<Page> {
    Page::parse(markdown).map_err(|cause| {
        rejected(
            kind,
            if kind == Write::Capture {
                "text"
            } else {
                "markdown"
            },
            cause,
        )
    })
}
pub(super) fn evidence(text: &str, source: &str) -> Result<()> {
    for (field, value, limit) in [
        ("text", text, EVIDENCE_TEXT_BYTES),
        ("source", source, SOURCE_BYTES),
    ] {
        if !nonblank(value, limit) {
            return Err(rejected(Write::Append, field, Error::invalid_evidence()));
        }
    }
    Ok(())
}
pub(super) fn capture(text: &str) -> Result<()> {
    if !nonblank(text, EVIDENCE_TEXT_BYTES) {
        return Err(rejected(Write::Capture, "text", Error::invalid_capture()));
    }
    Ok(())
}
pub(super) fn summary_hash(
    summary: Option<&str>,
    expected: Option<&crate::values::ContentHash>,
) -> Result<()> {
    if summary.is_some() && expected.is_none() {
        return Err(Write::Append.error(Code::MissingArgument, "expected_hash", false));
    }
    Ok(())
}
pub(super) fn decision(slug: &Slug, reason: &str) -> Result<()> {
    let field = if !Prefix::is_review(slug) {
        Some("slug")
    } else if !nonblank(reason, DECISION_REASON_BYTES) {
        Some("reason")
    } else {
        None
    };
    if let Some(field) = field {
        return Err(rejected(Write::Decide, field, Error::invalid_decision()));
    }
    Ok(())
}
pub(super) fn consolidation(request: &super::Consolidate, run: Option<&str>) -> Result<()> {
    let field = if !Prefix::is_inbox(&request.note) {
        Some("note")
    } else if Prefix::is_inbox(&request.target) || Prefix::is_review(&request.target) {
        Some("target")
    } else if run.is_none_or(str::is_empty) {
        Some("run")
    } else if request.summary.trim().is_empty() {
        Some("summary")
    } else {
        None
    };
    if let Some(field) = field {
        return Err(rejected(
            Write::Consolidate,
            field,
            Error::invalid_consolidation(),
        ));
    }
    Ok(())
}
