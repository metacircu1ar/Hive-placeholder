//! Prepare bounded user content before state, service, configuration or socket work.
mod context;
mod failure;
mod reader;
#[cfg(test)]
mod tests;
pub(super) use context::Context;
pub(super) use failure::Failure;
pub(super) type Result<T> = std::result::Result<T, Failure>;
use super::{BATCH_BYTES, Error, Path};

pub(super) fn body(command: &'static str, flag: &'static str, path: &Path) -> Result<String> {
    Context::new(command, flag)
        .file(path)
        .text(super::BODY_BYTES, false)
}

/// Exclusive inline/file alternatives share one exact-text contract.
pub(super) fn text_source(
    command: &'static str,
    inline_flag: &'static str,
    file_flag: &'static str,
    inline: Option<&str>,
    file: Option<&Path>,
) -> Result<String> {
    match (inline, file) {
        (Some(text), None) => Context::new(command, inline_flag).checked_text(
            text.to_owned(),
            super::BODY_BYTES,
            false,
        ),
        (None, Some(path)) => body(command, file_flag, path),
        _ => Err(Context::new(command, inline_flag).invalid(
            hive_input::Code::ConflictingArguments,
            "Choose one text source.",
            &format!("{inline_flag} or {file_flag}"),
        )),
    }
}

pub(super) fn batch_operations(path: &Path) -> Result<serde_json::Value> {
    let context = Context::new("batch", "--operations-file").file(path);
    let data = context.bytes(BATCH_BYTES)?;
    let mut operations = context.json(&data, BATCH_BYTES)?;
    let items = operations.as_array_mut().ok_or_else(|| {
        context.invalid(
            hive_input::Code::InvalidJson,
            "Batch operations must be a JSON array.",
            "a JSON array of operations",
        )
    })?;
    crate::runtime::api::batch::count(items.len())
        .map_err(|error| super::batch::local(error, Some(path)))?;
    for (index, item) in items.iter().enumerate() {
        crate::runtime::api::batch::header(item).map_err(|error| {
            super::batch::local(
                crate::runtime::api::batch::at_index(error, index),
                Some(path),
            )
        })?;
    }
    for item in items {
        if !matches!(
            item.get("op").and_then(serde_json::Value::as_str),
            Some("spawn" | "send")
        ) {
            continue;
        }
        if let Some(paths) = item
            .get_mut("attachments")
            .and_then(serde_json::Value::as_array_mut)
        {
            for path in paths {
                if let Some(text) = path.as_str() {
                    *path = serde_json::to_value(
                        std::path::absolute(text).map_err(Error::state_io_from)?,
                    )
                    .map_err(|cause| {
                        Error::attachment_invalid("Attachment path must be valid UTF-8.")
                            .with_source(cause)
                    })?;
                }
            }
        }
    }
    Ok(operations)
}

/// Absolutize only: accepted retries must not stat or canonicalize their sources.
pub(super) fn attachment_paths(
    command: &'static str,
    paths: &[std::path::PathBuf],
) -> Result<super::Input<Vec<std::path::PathBuf>>> {
    if paths.is_empty() {
        return Ok(super::Input::Missing);
    }
    if paths.len() > crate::runtime::attachments::MAX_FILES {
        return Err(Context::new(command, "--attach").invalid(
            hive_input::Code::OutOfRange,
            "Too many attachments.",
            "at most 10 attachment paths",
        ));
    }
    paths
        .iter()
        .map(|p| std::path::absolute(p).map_err(Error::state_io_from))
        .collect::<crate::error::Result<Vec<_>>>()
        .map(super::Input::Valid)
        .map_err(Failure::from)
}
