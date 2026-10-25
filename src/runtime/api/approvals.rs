//! Typed approval command arguments and response records.
use super::Input;
pub use crate::approvals::{ApprovalAction, ApprovalCursor};
use crate::error::Result;
use crate::runtime::limits::{
    CURSOR_ID_BYTES, DEFAULT_HISTORY_PAGE_ROWS, MAX_PAGE_ROWS, MIN_PAGE_ROWS,
};
use hive_input::Code;

mod ask;
mod validation;
pub(crate) use ask::AskContent;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::borrow::Cow;
pub(crate) use validation::ApprovalInput;

/// An object-shaped tool parameter even through provider schema sanitization.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ApprovalSchema(pub Map<String, Value>);
impl JsonSchema for ApprovalSchema {
    fn schema_name() -> Cow<'static, str> {
        "ApprovalSchema".into()
    }
    #[expect(
        clippy::expect_used,
        reason = "The serialized ObjectSchema has the object shape required by Schema."
    )]
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        #[derive(Serialize)]
        struct ObjectSchema {
            #[serde(rename = "type")]
            kind: &'static str,
            #[serde(rename = "additionalProperties")]
            additional_properties: bool,
        }
        Schema::try_from(super::value(ObjectSchema {
            kind: "object",
            additional_properties: true,
        }))
        .expect("object schema")
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct AskArgs {
    /// Question for the human, 1..16384 UTF-8 bytes; never request secrets.
    #[serde(default)]
    #[schemars(!default)]
    pub message: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    /// Optional nonempty display title, at most 256 UTF-8 bytes.
    pub title: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[doc = " Optional flat elicitation form: at most 16 fields, 64 choices per field and \
        16384 JSON bytes."]
    pub schema: Input<ApprovalSchema>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    /// Presentation hint, default false; not permission or enforcement.
    pub destructive: Input<bool>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ApprovalsArgs {
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub agent: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub all: Input<bool>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub after: Input<ApprovalCursor>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub limit: Input<u64>,
}
pub(crate) struct ApprovalOptions<'a> {
    pub agent: Option<&'a str>,
    pub all: bool,
    pub after: Option<&'a ApprovalCursor>,
    pub limit: u64,
}
impl ApprovalsArgs {
    pub(crate) fn options(&self) -> Result<ApprovalOptions<'_>> {
        let agent = match &self.agent {
            Input::Missing => None,
            Input::Valid(value)
                if !value.is_empty() && value.len() <= super::input::INPUT_STRING_BYTES =>
            {
                Some(value.as_str())
            }
            _ => {
                return Err(ApprovalInput::List.error(
                    Code::InvalidValue,
                    "/agent",
                    "a nonempty agent ID within 65536 UTF-8 bytes",
                ));
            }
        };
        let all = match self.all {
            Input::Missing => false,
            Input::Valid(value) => value,
            Input::Invalid(_) => {
                return Err(ApprovalInput::List.error(Code::InvalidValue, "/all", "a boolean"));
            }
        };
        let after = match &self.after {
            Input::Missing => None,
            Input::Valid(value) => {
                value.validate()?;
                Some(value)
            }
            Input::Invalid(_) => {
                return Err(ApprovalInput::List.error(
                    Code::InvalidValue,
                    "/after",
                    "an object with created_at (integer) and id (string)",
                ));
            }
        };
        let limit = match self.limit {
            Input::Missing => DEFAULT_HISTORY_PAGE_ROWS,
            Input::Valid(value) if (MIN_PAGE_ROWS..=MAX_PAGE_ROWS).contains(&value) => value,
            Input::Valid(_) => {
                return Err(ApprovalInput::List.error(
                    Code::OutOfRange,
                    "/limit",
                    "an integer from 1 to 200 rows",
                ));
            }
            Input::Invalid(_) => {
                return Err(ApprovalInput::List.error(
                    Code::InvalidValue,
                    "/limit",
                    "an integer from 1 to 200 rows",
                ));
            }
        };
        Ok(ApprovalOptions {
            agent,
            all,
            after,
            limit,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApprovalArgs {
    #[serde(default)]
    pub approval_id: Input<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RespondArgs {
    #[serde(default)]
    pub approval_id: Input<String>,
    #[serde(default)]
    pub action: Input<ApprovalAction>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub content: Input<Value>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub reason: Input<String>,
}

impl ApprovalCursor {
    pub(crate) fn validate(&self) -> Result<()> {
        if i64::try_from(self.created_at).is_err() {
            return Err(ApprovalInput::List.error(
                Code::OutOfRange,
                "/after/created_at",
                "an integer from 0 to 9223372036854775807",
            ));
        }
        if self.id.is_empty() || self.id.len() > CURSOR_ID_BYTES {
            return Err(ApprovalInput::List.error(
                Code::InvalidValue,
                "/after/id",
                "a nonempty cursor ID within 128 UTF-8 bytes",
            ));
        }
        Ok(())
    }
}
