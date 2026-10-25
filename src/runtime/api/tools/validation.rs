//! Validate only the generated hosted-tool vocabulary, without side effects.
use super::{TOOLS, catalog};
use crate::{
    error::{Error, Result},
    runtime::api::{AskArgs, BatchKind},
};
use hive_input::Code;
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

// Numeric domain rules remain at their existing consuming boundary.
#[derive(Deserialize)]
pub(super) struct ArgumentSchema {
    #[serde(default)]
    required: Vec<String>,
    #[serde(default)]
    properties: BTreeMap<String, Property>,
}
#[derive(Deserialize)]
struct Property {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(rename = "enum")]
    choices: Option<Vec<Value>>,
    #[serde(flatten)]
    object: ArgumentSchema,
}
#[derive(Deserialize)]
struct BatchHeader {
    op: BatchKind,
}

#[derive(Clone, Copy)]
struct Context<'a> {
    name: &'a str,
    prefix: &'a str,
}
impl Context<'_> {
    fn error(self, code: Code, field: &str, expected: &str) -> Error {
        let field = format!("{}{field}", self.prefix);
        catalog::error(
            self.name,
            code,
            if field.is_empty() { "/" } else { &field },
            expected,
        )
    }
}

pub(crate) fn validate_tool(name: &str, args: &Value) -> Result<()> {
    validate_arguments(name, args, Context { name, prefix: "" }, false)
}

fn validate_arguments(name: &str, args: &Value, context: Context<'_>, member: bool) -> Result<()> {
    let schema = TOOLS.arguments.get(name).ok_or_else(|| {
        context.error(
            Code::UnknownArgument,
            "/name",
            "an advertised Hive coordination tool",
        )
    })?;
    validate_object(name, schema, args, context, member)?;
    if name == "hive_ask" {
        // Only known, typed fields remain. The same content check runs after
        // replay/lifecycle validation in Core::ask for control callers.
        let ask = AskArgs::deserialize(args).map_err(|cause| {
            context
                .error(Code::InvalidValue, "", "an approval argument object")
                .with_source(cause)
        })?;
        ask.content()?;
    }
    Ok(())
}

fn validate_object(
    name: &str,
    schema: &ArgumentSchema,
    value: &Value,
    context: Context<'_>,
    member: bool,
) -> Result<()> {
    let args = value
        .as_object()
        .ok_or_else(|| context.error(Code::InvalidValue, "", "an object"))?;
    for key in &schema.required {
        if !args.contains_key(key) {
            let expected = schema
                .properties
                .get(key)
                .map_or("an advertised argument", |property| {
                    kind_expected(property.kind.as_deref())
                });
            return Err(context.error(Code::MissingArgument, &format!("/{key}"), expected));
        }
    }
    for (key, value) in args {
        if member && key == "op" {
            continue;
        }
        // Do not echo arbitrary keys, even as a JSON pointer.
        let property = schema.properties.get(key).ok_or_else(|| {
            context.error(
                Code::UnknownArgument,
                "",
                "only properties declared by this tool's schema",
            )
        })?;
        validate_property(name, key, property, value, context)?;
    }
    Ok(())
}

fn validate_property(
    name: &str,
    key: &str,
    property: &Property,
    value: &Value,
    context: Context<'_>,
) -> Result<()> {
    let field = format!("/{key}");
    let expected = kind_expected(property.kind.as_deref());
    let valid = match property.kind.as_deref() {
        Some("string") => value.is_string(),
        Some("integer") => value.as_u64().is_some(),
        Some("boolean") => value.is_boolean(),
        Some("object") => value.is_object(),
        Some("array") => value.is_array(),
        _ => false,
    };
    if !valid {
        return Err(context.error(Code::InvalidValue, &field, expected));
    }
    if let Some(choices) = &property.choices
        && !choices.contains(value)
    {
        return Err(context.error(
            Code::InvalidValue,
            &field,
            &format!("one of {}", crate::runtime::api::value(choices)),
        ));
    }
    match value {
        Value::String(text) => string(name, key, text, context),
        Value::Array(items) => array(key, items, context),
        Value::Object(_) if name == "hive_ask" && key == "schema" => Ok(()),
        Value::Object(_)
            if matches!(name, "hive_spawn" | "hive_set_options") && key == "options" =>
        {
            let options: crate::options::OptionMap = serde_json::from_value(value.clone())
                .map_err(|cause| {
                    context
                        .error(Code::InvalidValue, &field, "a catalog option object")
                        .with_source(cause)
                })?;
            crate::options::validate_unbound(&options, name == "hive_spawn").map_err(|error| {
                match &error {
                    Error::Input { input, .. } => context
                        .error(input.code(), input.field(), input.expected())
                        .with_source(error),
                    _ => error,
                }
            })
        }
        Value::Object(_) => object(name, key, property, value, context),
        _ => Ok(()),
    }
}

fn kind_expected(kind: Option<&str>) -> &'static str {
    match kind {
        Some("string") => "a string",
        Some("integer") => "an unsigned integer (not a fraction or null)",
        Some("boolean") => "a boolean",
        Some("object") => "an object",
        Some("array") => "an array",
        _ => "the advertised argument type",
    }
}

fn string(name: &str, key: &str, text: &str, context: Context<'_>) -> Result<()> {
    let field = format!("/{key}");
    if key == "request_id" {
        return hive_input::validate::request_id(text).map_err(|cause| {
            context
                .error(
                    Code::InvalidRequestId,
                    &field,
                    "1..128 UTF-8 bytes without control characters",
                )
                .with_source(cause)
        });
    }
    if (name == "hive_evidence" && matches!(key, "summary" | "commit"))
        || (name == "hive_ask" && matches!(key, "message" | "title"))
    {
        return Ok(());
    }
    if (text.is_empty() && key != "instructions") || text.len() > crate::limits::CONTROL_INPUT_BYTES
    {
        return Err(context.error(
            Code::InvalidValue,
            &field,
            if key == "instructions" {
                "0..65536 UTF-8 bytes; empty clears role instructions"
            } else {
                "1..65536 UTF-8 bytes"
            },
        ));
    }
    Ok(())
}

fn array(key: &str, items: &[Value], context: Context<'_>) -> Result<()> {
    match key {
        "operations" => {
            crate::runtime::api::batch::count(items.len())
                .map_err(|e| catalog::contextualize(e, context.name))?;
            for (index, operation) in items.iter().enumerate() {
                batch_member(
                    operation,
                    Context {
                        name: context.name,
                        prefix: &format!("/operations/{index}"),
                    },
                )?;
            }
        }
        "attachments" => {
            if items.len() > crate::runtime::attachments::MAX_FILES {
                return Err(context.error(
                    Code::OutOfRange,
                    "/attachments",
                    "an array of at most 10 absolute file paths",
                ));
            }
            for (index, value) in items.iter().enumerate() {
                if !value.as_str().is_some_and(|p| {
                    p.len() <= crate::limits::CONTROL_INPUT_BYTES
                        && !p.contains('\0')
                        && Path::new(p).is_absolute()
                }) {
                    return Err(context.error(
                        Code::InvalidPath,
                        &format!("/attachments/{index}"),
                        "an absolute file path within 65536 UTF-8 bytes without NUL",
                    ));
                }
            }
        }
        "files" => {
            // Evidence label semantics deliberately remain after replay.
            for (index, value) in items.iter().enumerate() {
                if !value.is_string() {
                    return Err(context.error(
                        Code::InvalidValue,
                        &format!("/files/{index}"),
                        "a string",
                    ));
                }
            }
        }
        _ => {
            return Err(context.error(
                Code::InvalidValue,
                &format!("/{key}"),
                "an advertised array argument",
            ));
        }
    }
    Ok(())
}

fn batch_member(operation: &Value, context: Context<'_>) -> Result<()> {
    let fields = operation
        .as_object()
        .ok_or_else(|| context.error(Code::InvalidValue, "", "an operation object"))?;
    let header = BatchHeader::deserialize(operation).map_err(|cause| {
        context
            .error(
                if fields.contains_key("op") {
                    Code::InvalidValue
                } else {
                    Code::MissingArgument
                },
                "/op",
                "spawn, send or terminate",
            )
            .with_source(cause)
    })?;
    let tool = match header.op {
        BatchKind::Spawn => "hive_spawn",
        BatchKind::Send => "hive_send",
        BatchKind::Terminate => "hive_terminate",
    };
    validate_arguments(tool, operation, context, true)
}

fn object(
    name: &str,
    key: &str,
    property: &Property,
    value: &Value,
    context: Context<'_>,
) -> Result<()> {
    let prefix = format!("{}/{key}", context.prefix);
    validate_object(
        name,
        &property.object,
        value,
        Context {
            name: context.name,
            prefix: &prefix,
        },
        false,
    )?;
    match (name, key) {
        ("hive_batch", "join") => {
            let options =
                crate::runtime::api::joins::JoinOptions::deserialize(value).map_err(|cause| {
                    context
                        .error(
                            Code::InvalidValue,
                            "/join",
                            "an object with optional deadline_seconds only",
                        )
                        .with_source(cause)
                })?;
            options
                .deadline(crate::state::now())
                .map_err(|e| catalog::contextualize(e, context.name))?;
        }
        ("hive_agents", "stalled_after") => {
            let cursor =
                crate::runtime::api::StalledCursor::deserialize(value).map_err(|cause| {
                    context
                        .error(
                            Code::InvalidValue,
                            "/stalled_after",
                            "a stalled-group cursor object",
                        )
                        .with_source(cause)
                })?;
            if cursor.validate().is_err() {
                if cursor.idle_since > i64::MAX as u64 {
                    return Err(context.error(
                        Code::OutOfRange,
                        "/stalled_after/idle_since",
                        "an integer from 0 to 9223372036854775807",
                    ));
                }
                return Err(context.error(
                    Code::InvalidValue,
                    "/stalled_after/root_id",
                    "a nonempty cursor ID within 128 UTF-8 bytes",
                ));
            }
        }
        _ => (),
    }
    Ok(())
}
