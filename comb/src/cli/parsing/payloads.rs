//! Scalar payload rules run before any file or stdin preparation.
use super::relations::rejected;
use crate::cli::args::RawCommand;
use comb::{
    store::input,
    values::{Prefix, Slug},
};
use hive_input::{Code, Rendered};

pub(super) fn validate(command: &RawCommand, path: &str) -> Result<(), Rendered> {
    match command {
        RawCommand::Append(args) => {
            text(path, "TEXT", &args.text, input::EVIDENCE_TEXT_BYTES)?;
            text(path, "--source", &args.source, input::SOURCE_BYTES)?;
        }
        RawCommand::Capture(args) => text(path, "TEXT", &args.text, input::EVIDENCE_TEXT_BYTES)?,
        RawCommand::Decide(args) => {
            text(path, "--reason", &args.reason, input::DECISION_REASON_BYTES)?;
            prefix(
                path,
                "SLUG",
                &args.slug,
                Prefix::is_review,
                "a descendant of review/",
            )?;
        }
        RawCommand::Consolidate(args) => {
            prefix(
                path,
                "NOTE",
                &args.note,
                Prefix::is_inbox,
                "a descendant of inbox/",
            )?;
            prefix(
                path,
                "TARGET",
                &args.target,
                |slug| !Prefix::is_inbox(slug) && !Prefix::is_review(slug),
                "a target outside inbox/ and review/",
            )?;
        }
        _ => {}
    }
    Ok(())
}

fn text(path: &str, field: &str, value: &str, max: usize) -> Result<(), Rendered> {
    if !input::nonblank(value, max) {
        return Err(rejected(
            path,
            Code::InvalidValue,
            field,
            &format!("nonblank text of at most {max} UTF-8 bytes"),
        ));
    }
    Ok(())
}

fn prefix(
    path: &str,
    field: &str,
    value: &str,
    predicate: impl FnOnce(&Slug) -> bool,
    expected: &str,
) -> Result<(), Rendered> {
    if !value.parse::<Slug>().is_ok_and(|slug| predicate(&slug)) {
        return Err(rejected(path, Code::InvalidValue, field, expected));
    }
    Ok(())
}
