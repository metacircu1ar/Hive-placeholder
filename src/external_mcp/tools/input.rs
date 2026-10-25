//! Side-effect-free decoding; protocol shape and tool input failures stay distinct.
use super::{CallArgs, ExternalTool, Operation, Output, ToolCall, WaitArgs};
use crate::{
    calls,
    error::{Error, Result},
    external_mcp::{catalog, wire},
    limits::CONTROL_INPUT_BYTES,
    runtime::domain::DeliveryMode,
};
use hive_input::{Code, validate};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use std::path::PathBuf;

#[derive(Debug)]
pub(in crate::external_mcp) enum ParseError {
    Protocol(Error),
    Arguments(Error),
}

pub(in crate::external_mcp) fn parse(params: Value) -> std::result::Result<ToolCall, ParseError> {
    let (context, metadata) = wrapper(params).map_err(ParseError::Protocol)?;
    let progress_token = wire::progress_token(metadata).map_err(|cause| {
        ParseError::Protocol(
            context
                .error(
                    Code::InvalidValue,
                    "/_meta/progressToken",
                    "a string or number",
                )
                .with_source(cause),
        )
    })?;
    let operation = context.operation().map_err(ParseError::Arguments)?;
    Ok(ToolCall {
        operation,
        progress_token,
    })
}

fn wrapper(params: Value) -> Result<(Context, Value)> {
    let generic = Context {
        name: ExternalTool::Other,
        args: Map::new(),
    };
    let mut fields: Map<String, Value> = serde_json::from_value(params).map_err(|cause| {
        generic
            .error(Code::InvalidValue, "/", "a tools/call params object")
            .with_source(cause)
    })?;
    let name = fields.remove("name").ok_or_else(|| {
        generic.error(
            Code::MissingArgument,
            "/name",
            "hive_call, hive_wait_message or hive_output",
        )
    })?;
    let name: ExternalTool = serde_json::from_value(name).map_err(|cause| {
        generic
            .error(
                Code::InvalidValue,
                "/name",
                "hive_call, hive_wait_message or hive_output",
            )
            .with_source(cause)
    })?;
    if name == ExternalTool::Other {
        return Err(generic.error(
            Code::UnknownArgument,
            "/name",
            "hive_call, hive_wait_message or hive_output",
        ));
    }
    let mut context = Context {
        name,
        args: Map::new(),
    };
    if let Some(arguments) = fields.remove("arguments") {
        context.args = serde_json::from_value(arguments).map_err(|cause| {
            context
                .error(Code::InvalidValue, "/arguments", "an object")
                .with_source(cause)
        })?;
    }
    Ok((context, fields.remove("_meta").unwrap_or_default()))
}

struct Context {
    name: ExternalTool,
    args: Map<String, Value>,
}
impl Context {
    fn error(&self, code: Code, field: &str, expected: &str) -> Error {
        hive_input::InputError::new(
            code,
            "Invalid external tool arguments.",
            field,
            expected,
            vec![catalog::example(self.name).into()],
        )
        .map_or_else(
            |cause| Error::internal_error("Invalid tool input metadata.").with_source(cause),
            Error::from,
        )
    }
    fn argument(&self, code: Code, key: &str, expected: &str) -> Error {
        self.error(code, &format!("/arguments/{key}"), expected)
    }
    fn decode<T: DeserializeOwned>(&self, key: &str, default: Option<T>) -> Result<T> {
        match self.args.get(key) {
            Some(value) => serde_json::from_value(value.clone()).map_err(|cause| {
                self.argument(Code::InvalidValue, key, catalog::expected(key))
                    .with_source(cause)
            }),
            None => default
                .ok_or_else(|| self.argument(Code::MissingArgument, key, catalog::expected(key))),
        }
    }
    fn operation(&self) -> Result<Operation> {
        // Required keys first, then unknown keys, then values in fixed catalog order.
        for key in catalog::required(self.name) {
            if !self.args.contains_key(*key) {
                return Err(self.argument(Code::MissingArgument, key, catalog::expected(key)));
            }
        }
        if self.args.keys().any(|key| {
            !catalog::required(self.name).contains(&key.as_str())
                && !catalog::optional(self.name).contains(&key.as_str())
        }) {
            return Err(self.error(
                Code::UnknownArgument,
                "/arguments",
                "only properties declared by this tool's schema",
            ));
        }
        match self.name {
            ExternalTool::Call => self.call().map(Operation::Call),
            ExternalTool::WaitMessage => Ok(Operation::Wait(WaitArgs {
                message_id: self.text("message_id")?,
                timeout: self.timeout()?,
            })),
            ExternalTool::Output => self.output().map(Operation::Output),
            ExternalTool::Other => Err(self.error(
                Code::UnknownArgument,
                "/name",
                "an advertised external tool",
            )),
        }
    }
    fn text(&self, key: &str) -> Result<String> {
        let value: String = self.decode(key, None)?;
        if value.is_empty() || value.len() > CONTROL_INPUT_BYTES {
            return Err(self.argument(Code::OutOfRange, key, catalog::expected(key)));
        }
        Ok(value)
    }
    fn timeout(&self) -> Result<u64> {
        let value = self.decode("timeout", Some(calls::DEFAULT_TIMEOUT))?;
        if !(1..=calls::MAX_TIMEOUT).contains(&value) {
            return Err(self.argument(Code::OutOfRange, "timeout", catalog::expected("timeout")));
        }
        Ok(value)
    }
    fn call(&self) -> Result<CallArgs> {
        let target = self.text("target")?;
        let body = self.text("body")?;
        let request_id: String = self.decode("request_id", None)?;
        validate::request_id(&request_id).map_err(|cause| {
            self.argument(cause.code(), "request_id", cause.expected())
                .with_source(cause)
        })?;
        let attachments = self.attachments()?;
        let mode = self.decode("mode", Some(DeliveryMode::NextTurn))?;
        if !matches!(mode, DeliveryMode::NextTurn | DeliveryMode::Redirect) {
            return Err(self.argument(Code::InvalidValue, "mode", catalog::expected("mode")));
        }
        Ok(CallArgs {
            target,
            body,
            request_id,
            attachments,
            mode,
            timeout: self.timeout()?,
        })
    }
    fn attachments(&self) -> Result<Vec<PathBuf>> {
        let values: Vec<Value> = self.decode("attachments", Some(vec![]))?;
        if values.len() > crate::runtime::attachments::MAX_FILES {
            return Err(self.argument(
                Code::OutOfRange,
                "attachments",
                catalog::expected("attachments"),
            ));
        }
        values
            .into_iter()
            .enumerate()
            .map(|(index, value)| {
                let invalid = || {
                    self.argument(
                        Code::InvalidPath,
                        &format!("attachments/{index}"),
                        "an absolute path within 1..65536 UTF-8 bytes, without NUL",
                    )
                };
                let text: String =
                    serde_json::from_value(value).map_err(|cause| invalid().with_source(cause))?;
                let path = PathBuf::from(&text);
                if text.len() > CONTROL_INPUT_BYTES || text.contains('\0') || !path.is_absolute() {
                    return Err(invalid());
                }
                Ok(path)
            })
            .collect()
    }
    fn output(&self) -> Result<Output> {
        let id: String = self.decode("id", None)?;
        validate::uuid(&id).map_err(|cause| {
            self.argument(cause.code(), "id", cause.expected())
                .with_source(cause)
        })?;
        let offset = self.decode("offset", Some(0))?;
        let limit = self.decode("limit", Some(CONTROL_INPUT_BYTES as u64))?;
        crate::runtime::api::pagination::bytes(limit).map_err(|cause| {
            self.argument(Code::OutOfRange, "limit", catalog::expected("limit"))
                .with_source(cause)
        })?;
        Ok(Output { id, offset, limit })
    }
}
