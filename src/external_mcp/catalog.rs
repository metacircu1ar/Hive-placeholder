//! Own tool definitions serialize in the same sorted key order as JSON values.
mod descriptions;
use super::tools::ExternalTool;
use crate::{
    calls,
    limits::{CONTROL_INPUT_BYTES, REQUEST_ID_BYTES},
};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Definition {
    description: String,
    input_schema: Object,
    name: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Object {
    additional_properties: bool,
    properties: BTreeMap<&'static str, Property>,
    required: &'static [&'static str],
    #[serde(rename = "type")]
    kind: &'static str,
}

// Field order preserves the exact sorted wire representation.
#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Property {
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<DefaultValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'static str>,
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    choices: Option<[&'static str; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    items: Option<Box<Property>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_items: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_length: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    maximum: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_length: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    minimum: Option<u64>,
    #[serde(rename = "type")]
    kind: &'static str,
}
#[derive(Serialize)]
#[serde(untagged)]
enum DefaultValue {
    Number(u64),
    Text(&'static str),
}
impl Property {
    fn described(mut self, description: &'static str) -> Self {
        self.description = Some(description);
        self
    }
    fn attachments() -> Self {
        Self {
            kind: "array",
            items: Some(Box::new(
                Self::string(CONTROL_INPUT_BYTES as u64)
                    .described("Absolute source path within 1..65536 UTF-8 bytes, without NUL."),
            )),
            max_items: Some(crate::runtime::attachments::MAX_FILES as u64),
            ..Self::default()
        }
    }
    fn string(max_length: u64) -> Self {
        Self {
            max_length: Some(max_length),
            min_length: Some(1),
            kind: "string",
            ..Self::default()
        }
    }
    fn integer(minimum: u64, maximum: Option<u64>, default: Option<u64>) -> Self {
        Self {
            default: default.map(DefaultValue::Number),
            maximum,
            minimum: Some(minimum),
            kind: "integer",
            ..Self::default()
        }
    }
    fn timeout() -> Self {
        Self::integer(1, Some(calls::MAX_TIMEOUT), Some(calls::DEFAULT_TIMEOUT))
    }
    fn mode() -> Self {
        Self {
            default: Some(DefaultValue::Text("next-turn")),
            choices: Some(["next-turn", "redirect"]),
            kind: "string",
            ..Self::default()
        }
    }
}

impl Definition {
    fn new<const N: usize>(
        name: &'static str,
        description: &'static str,
        required: &'static [&'static str],
        properties: [(&'static str, Property); N],
    ) -> Self {
        Self {
            description: format!(
                "{description} Example: {}",
                example(ExternalTool::parse(name))
            ),
            input_schema: Object {
                additional_properties: false,
                properties: BTreeMap::from(properties),
                required,
                kind: "object",
            },
            name,
        }
    }
}

pub(super) fn catalog() -> Vec<Definition> {
    vec![call(), wait_message(), output()]
}

fn call() -> Definition {
    Definition::new(
        "hive_call",
        descriptions::HIVE_CALL_DESCRIPTION,
        required(ExternalTool::Call),
        [
            (
                "attachments",
                Property::attachments().described(
                    "Ordered absolute source paths, \
                up to 10; default empty. Captured when the send is accepted.",
                ),
            ),
            (
                "target",
                Property::string(CONTROL_INPUT_BYTES as u64).described(
                    "Recipient agent \
                ID, 1..65536 UTF-8 bytes.",
                ),
            ),
            (
                "body",
                Property::string(CONTROL_INPUT_BYTES as u64).described(
                    "Message body, \
                1..65536 UTF-8 bytes; newlines are allowed.",
                ),
            ),
            (
                "request_id",
                Property::string(REQUEST_ID_BYTES as u64).described(
                    "Retry \
                identity, 1..128 UTF-8 bytes without control characters; reuse it with \
                identical send arguments.",
                ),
            ),
            (
                "mode",
                Property::mode().described(
                    "next-turn (default) or redirect; steer is not \
                supported.",
                ),
            ),
            (
                "timeout",
                Property::timeout().described(
                    "Observation timeout in integer seconds, \
                1..600; default 45. Does not cancel the message.",
                ),
            ),
        ],
    )
}

fn wait_message() -> Definition {
    Definition::new(
        "hive_wait_message",
        descriptions::HIVE_WAIT_MESSAGE_DESCRIPTION,
        required(ExternalTool::WaitMessage),
        [
            (
                "message_id",
                Property::string(CONTROL_INPUT_BYTES as u64).described(
                    "Previously \
                accepted message ID, 1..65536 UTF-8 bytes.",
                ),
            ),
            (
                "timeout",
                Property::timeout().described(
                    "Observation timeout in integer seconds, \
                1..600; default 45. Does not cancel the message.",
                ),
            ),
        ],
    )
}

fn output() -> Definition {
    Definition::new(
        "hive_output",
        descriptions::HIVE_OUTPUT_DESCRIPTION,
        required(ExternalTool::Output),
        [
            (
                "id",
                Property::string(CONTROL_INPUT_BYTES as u64)
                    .described("UUID of an output artifact returned by a call result."),
            ),
            (
                "offset",
                Property::integer(0, None, None).described(
                    "Byte offset, an unsigned integer; default 0. Continue using next_offset.",
                ),
            ),
            (
                "limit",
                Property::integer(
                    crate::runtime::limits::MIN_OUTPUT_BYTES,
                    Some(crate::runtime::limits::MAX_OUTPUT_BYTES),
                    Some(CONTROL_INPUT_BYTES as u64),
                )
                .described(
                    "Maximum UTF-8 page size in bytes, an integer from 4 to 65536; default 65536.",
                ),
            ),
        ],
    )
}

// These trusted field lists fix diagnostic precedence independently of JSON key order.
pub(super) fn required(name: ExternalTool) -> &'static [&'static str] {
    match name {
        ExternalTool::Call => &["target", "body", "request_id"],
        ExternalTool::WaitMessage => &["message_id"],
        ExternalTool::Output => &["id"],
        ExternalTool::Other => &[],
    }
}
pub(super) fn optional(name: ExternalTool) -> &'static [&'static str] {
    match name {
        ExternalTool::Call => &["attachments", "mode", "timeout"],
        ExternalTool::WaitMessage => &["timeout"],
        ExternalTool::Output => &["offset", "limit"],
        ExternalTool::Other => &[],
    }
}
pub(super) fn example(name: ExternalTool) -> &'static str {
    match name {
        ExternalTool::WaitMessage => {
            r#"{"name":"hive_wait_message","arguments":{"message_id":"MESSAGE_ID","timeout":45}}"#
        }
        ExternalTool::Output => {
            "{\"name\":\"hive_output\",\"arguments\":{\"id\":\
            \"00000000-0000-4000-8000-000000000001\",\"offset\":0,\"limit\":65536}}"
        }
        _ => {
            "{\"name\":\"hive_call\",\"arguments\":{\"target\":\"AGENT_ID\",\"body\":\
            \"Summarize the current diff.\",\"request_id\":\"summary-1\",\"timeout\":45}}"
        }
    }
}
pub(super) fn expected(key: &str) -> &'static str {
    match key {
        "target" | "body" | "message_id" => "a string within 1..65536 UTF-8 bytes",
        "request_id" => "1..128 UTF-8 bytes without control characters",
        "attachments" => "an array of up to 10 absolute path strings",
        "mode" => "next-turn or redirect",
        "timeout" => "an integer from 1 to 600 seconds",
        "id" => "a UUID string",
        "offset" => "an integer from 0 to 18446744073709551615 bytes",
        "limit" => "an integer from 4 to 65536 bytes",
        _ => "an advertised argument",
    }
}
