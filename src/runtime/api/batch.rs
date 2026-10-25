//! Pure batch checks; item validation stays sequential at the consuming boundary.
use super::{BatchArgs, BatchItem, Input, joins::JoinOptions};
use crate::error::{Error, Result};
use hive_input::Code;
use serde_json::Value;

pub(crate) fn error(code: Code, field: impl Into<String>, expected: &str) -> Error {
    hive_input::InputError::new(
        code,
        "Invalid batch arguments.",
        field,
        expected,
        vec![
            "{\"op\":\"batch\",\"operations\":[{\"op\":\"send\",\"target\":\"AGENT_ID\",\
            \"body\":\"Review the work.\",\"request_id\":\"review-1\"}]}"
                .into(),
        ],
    )
    .map_or_else(
        |cause| Error::internal_error("Invalid batch input metadata.").with_source(cause),
        Error::from,
    )
}

/// Preserve operational errors and retained causes; only input fields gain context.
pub(crate) fn at_index(mut error: Error, index: usize) -> Error {
    if let Error::Input { input, .. } = &mut error {
        match hive_input::InputError::new(
            input.code(),
            input.message(),
            format!(
                "/operations/{index}{}",
                if input.field() == "/" {
                    ""
                } else {
                    input.field()
                }
            ),
            input.expected(),
            vec![
                "{\"op\":\"batch\",\"operations\":[{\"op\":\"send\",\"target\":\"AGENT_ID\",\
                \"body\":\"Review the work.\",\"request_id\":\"review-1\"}]}"
                    .into(),
            ],
        ) {
            Ok(mapped) => **input = mapped,
            Err(cause) => {
                return Error::internal_error("Invalid batch input metadata.").with_source(cause);
            }
        }
    }
    error
}

pub(crate) fn count(length: usize) -> Result<()> {
    if (1..=crate::runtime::limits::BATCH_OPERATION_LIMIT).contains(&length) {
        Ok(())
    } else {
        Err(error(
            Code::OutOfRange,
            "/operations",
            "an array of 1 to 64 operations",
        ))
    }
}

pub(crate) fn size(bytes: usize) -> Result<()> {
    if bytes <= crate::runtime::limits::BATCH_INPUT_BYTES {
        Ok(())
    } else {
        Err(error(
            Code::OutOfRange,
            "/operations",
            "a complete serialized batch request within 524288 bytes",
        ))
    }
}

pub(crate) fn header(value: &Value) -> Result<&str> {
    let fields = value
        .as_object()
        .ok_or_else(|| error(Code::InvalidValue, "/", "an operation object"))?;
    let id = fields.get("request_id").ok_or_else(|| {
        error(
            Code::MissingArgument,
            "/request_id",
            "a request_id string of 1 to 128 UTF-8 bytes without control characters",
        )
    })?;
    let id = id
        .as_str()
        .filter(|id| hive_input::validate::request_id(id).is_ok())
        .ok_or_else(|| {
            error(
                Code::InvalidRequestId,
                "/request_id",
                "a request_id string of 1 to 128 UTF-8 bytes without control characters",
            )
        })?;
    match fields.get("op") {
        None => Err(error(
            Code::MissingArgument,
            "/op",
            "spawn, send or terminate",
        )),
        Some(Value::String(op)) if matches!(op.as_str(), "spawn" | "send" | "terminate") => Ok(id),
        _ => Err(error(Code::InvalidValue, "/op", "spawn, send or terminate")),
    }
}

impl BatchArgs {
    pub(crate) fn items(&self) -> Result<&[BatchItem]> {
        let items = match &self.operations {
            Input::Missing => {
                return Err(error(
                    Code::MissingArgument,
                    "/operations",
                    "an array of 1 to 64 operations",
                ));
            }
            Input::Invalid(_) => {
                return Err(error(
                    Code::InvalidJson,
                    "/operations",
                    "an array of 1 to 64 operations",
                ));
            }
            Input::Valid(items) => items,
        };
        count(items.len())?;
        Ok(items)
    }

    pub(crate) fn join_options(&self) -> Result<Option<&JoinOptions>> {
        match &self.join {
            Input::Missing => Ok(None),
            Input::Valid(options) => Ok(Some(options)),
            Input::Invalid(_) => Err(error(
                Code::InvalidJson,
                "/join",
                "an object with optional deadline_seconds only",
            )),
        }
    }
}
