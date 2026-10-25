//! Bounded answers input and trusted source coordinates; received values stay private.
use super::{Resolved, Result, Values};
use crate::{cli::input::Context, error::Error};
use hive_input::{Code, InputError};
use serde_yaml_ng::{Mapping, Value};
use std::path::Path;

fn invalid(expected: &str) -> Error {
    Context::new("setup", "--answers")
        .invalid(Code::InvalidFile, "Invalid setup answers.", expected)
        .error
}

pub(super) fn read(path: &Path) -> Result<serde_json::Value> {
    let text = super::super::config::read_required_text(path).map_err(file_error)?;
    let yaml = super::super::config::strict_yaml(&text).map_err(|cause| {
        invalid("a YAML mapping without anchors, aliases, tags, duplicate or merge keys")
            .with_source(cause)
    })?;
    let fields = yaml
        .as_mapping()
        .ok_or_else(|| invalid("a YAML mapping of setup option names to values"))?;
    let known = encoded(&Values::default())?;
    for (key, value) in fields {
        let name = key
            .as_str()
            .filter(|name| known.get(*name).is_some())
            .ok_or_else(|| invalid("only documented setup option keys"))?;
        let one = Value::Mapping(Mapping::from_iter([(key.clone(), value.clone())]));
        serde_yaml_ng::from_value::<Values>(one).map_err(|cause| {
            invalid(&format!(
                "key '{name}': a value of the documented option type"
            ))
            .with_source(cause)
        })?;
    }
    let parsed: Values = serde_yaml_ng::from_value(yaml)
        .map_err(|cause| invalid("a strict setup option mapping").with_source(cause))?;
    encoded(&parsed)
}

fn file_error(cause: Error) -> Error {
    use std::error::Error as _;
    if let Some(io) = cause
        .source()
        .and_then(|source| source.downcast_ref::<std::io::Error>())
        && !crate::os::input_path_error(io)
        && io.kind() != std::io::ErrorKind::InvalidData
    {
        return cause;
    }
    invalid(
        "an existing readable owned regular UTF-8 file up to 1 MiB, not writable by other users",
    )
    .with_source(cause)
}

pub(super) fn encoded(value: &Values) -> Result<serde_json::Value> {
    serde_json::to_value(value)
        .map_err(|cause| Error::internal_error("Cannot encode setup options.").with_source(cause))
}

impl Resolved {
    pub(in crate::cli::setup) fn attribute(&self, error: Error) -> Error {
        let Error::Input { input, .. } = &error else {
            return error;
        };
        let Some(key) = input.field().strip_prefix("--") else {
            return error;
        };
        if !self.answer_fields.contains(key) {
            return error;
        }
        // The key is admitted by Values, never copied from an unknown YAML key.
        match InputError::new(
            input.code(),
            "Invalid setup answer.",
            "--answers",
            format!("key '{key}': {}", input.expected()),
            input.examples().to_vec(),
        ) {
            Ok(input) => Error::from(input).with_source(error),
            Err(cause) => Error::internal_error("Invalid setup error metadata.").with_source(cause),
        }
    }
}
