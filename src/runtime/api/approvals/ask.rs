//! Shared approval content checks; lifecycle and replay precede these in the core.
use super::{AskArgs, Input};
use crate::{approvals, error::Result, runtime::api::tool_input_error};
use hive_input::Code;
use serde_json::Value;

pub(crate) struct AskContent<'a> {
    pub message: &'a str,
    pub title: Option<String>,
    pub schema: Option<Value>,
    pub destructive: bool,
}

impl AskArgs {
    pub(crate) fn content(&self) -> Result<AskContent<'_>> {
        let message = text(&self.message, "/message", approvals::MESSAGE_BYTES)?;
        let title = match &self.title {
            Input::Missing => None,
            value => Some(text(value, "/title", approvals::TITLE_BYTES)?.to_owned()),
        };
        let schema = match &self.schema {
            Input::Missing => None,
            Input::Valid(schema) => {
                let value = Value::Object(schema.0.clone());
                if !approvals::valid_schema(&value) {
                    return Err(schema_error());
                }
                Some(value)
            }
            Input::Invalid(_) => return Err(schema_error()),
        };
        let destructive = match self.destructive {
            Input::Missing => false,
            Input::Valid(value) => value,
            Input::Invalid(_) => {
                return Err(tool_input_error(
                    "hive_ask",
                    Code::InvalidValue,
                    "/destructive",
                    "a boolean",
                ));
            }
        };
        Ok(AskContent {
            message,
            title,
            schema,
            destructive,
        })
    }
}

fn text<'a>(value: &'a Input<String>, field: &str, cap: usize) -> Result<&'a str> {
    let code = match value {
        Input::Missing => Code::MissingArgument,
        Input::Valid(text) if !text.is_empty() && text.len() <= cap => return Ok(text),
        Input::Valid(text) if text.len() > cap => Code::OutOfRange,
        _ => Code::InvalidValue,
    };
    Err(tool_input_error(
        "hive_ask",
        code,
        field,
        &format!("1..{cap} UTF-8 bytes"),
    ))
}

fn schema_error() -> crate::error::Error {
    tool_input_error(
        "hive_ask",
        Code::InvalidValue,
        "/schema",
        "a flat elicitation form object with at most 16 fields, 64 choices per \
            field and 16384 serialized JSON bytes",
    )
}
