//! Runtime command envelopes and argument records.
mod dispatch;
pub use dispatch::Request;
mod spawn;
pub use spawn::SpawnArgs;

use super::super::invalid_request;
use super::input::INPUT_STRING_BYTES;
use super::{ApprovalArgs, ApprovalsArgs, AskArgs, Input, InputContext as Op, RespondArgs};
use crate::error::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// Long-poll live items for an attached agent.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LiveArgs {
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub target: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub after: Input<crate::runtime::live::Cursor>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub wait_ms: Input<u64>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub limit: Input<u64>,
}
/// Long-poll a compact agent tree, or the live root agents without `root`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TreeArgs {
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub root: Input<String>,
    /// Every live agent rather than the live roots; excludes `root`.
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub all: Input<bool>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub after: Input<crate::runtime::live::TreeVersion>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub wait_ms: Input<u64>,
}
/// Open a live channel and return one consistent snapshot of an agent, with the
/// newest event page, the page after a durable cursor, or the page before one.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AttachArgs {
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub target: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub limit: Input<u64>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub after: Input<u64>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub before: Input<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PurgeArgs {
    #[serde(default)]
    pub target: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub recursive: Input<bool>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub dry_run: Input<bool>,
}
pub(crate) struct PurgeOptions {
    pub recursive: bool,
    pub dry_run: bool,
}
impl PurgeArgs {
    pub(crate) fn options(&self) -> Result<PurgeOptions> {
        Ok(PurgeOptions {
            recursive: self
                .recursive
                .optional(Op::Purge, "/recursive", "a boolean")?
                .copied()
                .unwrap_or(false),
            dry_run: self
                .dry_run
                .optional(Op::Purge, "/dry_run", "a boolean")?
                .copied()
                .unwrap_or(false),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct GcCursor {
    pub ended_at: u64,
    pub id: String,
}
impl<'de> Deserialize<'de> for GcCursor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            ended_at: u64,
            id: String,
        }
        let Fields { ended_at, id } = crate::runtime::domain::decode::object(deserializer)?;
        Ok(Self { ended_at, id })
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GcArgs {
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub before: Input<u64>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub after: Input<GcCursor>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub dry_run: Input<bool>,
}
pub(crate) struct GcOptions<'a> {
    pub before: u64,
    pub after: Option<&'a GcCursor>,
    pub dry_run: bool,
}
impl GcArgs {
    pub(crate) fn options(&self) -> Result<GcOptions<'_>> {
        use hive_input::Code;
        let before = self
            .before
            .optional(
                Op::Gc,
                "/before",
                "an integer from 0 to 9223372036854775807",
            )?
            .copied()
            .ok_or_else(|| {
                Op::Gc.error(
                    Code::MissingArgument,
                    "/before",
                    "an integer from 0 to 9223372036854775807",
                )
            })?;
        if before > i64::MAX as u64 {
            return Err(Op::Gc.error(
                Code::OutOfRange,
                "/before",
                "an integer from 0 to 9223372036854775807",
            ));
        }
        let after = self.after.optional(
            Op::Gc,
            "/after",
            "an object with ended_at (integer) and id (string)",
        )?;
        if let Some(cursor) = after {
            if cursor.ended_at >= before {
                return Err(Op::Gc.error(
                    Code::OutOfRange,
                    "/after/ended_at",
                    "an integer from 0 to before - 1",
                ));
            }
            hive_input::validate::text_bytes(&cursor.id, INPUT_STRING_BYTES)
                .map_err(|rule| Op::Gc.error(rule.code(), "/after/id", rule.expected()))?;
        }
        let dry_run = self
            .dry_run
            .optional(Op::Gc, "/dry_run", "a boolean")?
            .copied()
            .unwrap_or(false);
        Ok(GcOptions {
            before,
            after,
            dry_run,
        })
    }
}

/// Never reconstruct this raw value from the typed request: replay fingerprints
/// include unknown keys and explicit null/default fields from the original input.
#[derive(Clone, Debug)]
pub(crate) struct RequestEnvelope {
    pub request: Request,
    raw: Value,
}
impl RequestEnvelope {
    pub(crate) fn batch_items(&self) -> Option<&[BatchItem]> {
        self.checked_batch_items().ok()
    }

    pub(crate) fn checked_batch_items(&self) -> Result<&[BatchItem]> {
        let Request::Batch(batch) = &self.request else {
            return Err(invalid_request());
        };
        let items = batch.items()?;
        super::batch::size(self.fingerprint().len())?;
        Ok(items)
    }

    pub fn new(raw: Value) -> Self {
        // Bound the raw envelope's discriminator before decoding arguments. Keep
        // the envelope itself intact for exact replay fingerprints.
        let request = if raw["op"]
            .as_str()
            .is_some_and(|op| !op.is_empty() && op.len() <= INPUT_STRING_BYTES)
        {
            serde_json::from_value(raw.clone()).unwrap_or(Request::Invalid)
        } else {
            Request::Invalid
        };
        Self { request, raw }
    }
    pub fn fingerprint(&self) -> String {
        self.raw.to_string()
    }
}

/// Batch items keep their raw fields until their own sequential validation and
/// dispatch. Decoding the outer batch must not validate a later item early.
#[derive(Clone, Debug, Serialize)]
#[serde(transparent)]
pub struct BatchItem(Value);
impl<'de> Deserialize<'de> for BatchItem {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        Value::deserialize(deserializer).map(Self)
    }
}
pub(crate) struct BatchOperation {
    pub id: String,
    pub request: RequestEnvelope,
}

impl BatchItem {
    #[expect(
        clippy::expect_used,
        reason = "Batch header validation requires an object before removing its request_id."
    )]
    pub(crate) fn operation(&self) -> Result<BatchOperation> {
        let id = super::batch::header(&self.0)?;
        let mut raw = self.0.clone();
        raw.as_object_mut()
            .expect("validated request envelope is an object")
            .remove("request_id");
        Ok(BatchOperation {
            id: id.to_owned(),
            request: RequestEnvelope::new(raw),
        })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BatchKind {
    Spawn,
    Send,
    Terminate,
}

// Requiredness describes the public tool contract. Serde defaults defer the
// socket validation; schemars must not treat those defaults as optional fields.

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct AgentsArgs {
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[schemars(range(min = 1))]
    /// List quiet live groups after this many seconds; cannot combine with after, parent or auth.
    pub stalled_seconds: Input<u64>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    /// Cursor from `next_cursor` in the previous stalled-group page.
    pub stalled_after: Input<super::StalledCursor>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[schemars(range(min = 0))]
    /// Agent-list sequence cursor, default 0; use `next_after` for the next page.
    pub after: Input<u64>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[schemars(range(min = 1))]
    /// Maximum returned rows/groups, 1..200; default 200 agents or 100 stalled groups.
    pub limit: Input<u64>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    /// Filter by delegating parent agent ID.
    pub parent: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    /// Filter by current bound login profile or assigned fallback group, not a pending switch.
    pub auth: Input<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct StatusArgs {
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[schemars(!default, !skip_serializing_if)]
    pub target: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub instructions: Input<bool>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ReadArgs {
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[schemars(!default, !skip_serializing_if)]
    /// Agent ID, artifact:ID for large output, or join:ID for a retained join snapshot.
    pub target: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[schemars(range(min = 0))]
    /// Event sequence cursor or artifact byte offset, default 0; omit for joins.
    pub after: Input<u64>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[schemars(range(min = 1))]
    /// 1..200 event rows (default 100) or 4..65536 artifact bytes (default 65536); omit for joins.
    pub limit: Input<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct OutputArgs {
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[schemars(!default, !skip_serializing_if)]
    pub id: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[schemars(range(min = 0))]
    pub offset: Input<u64>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[schemars(range(min = 4))]
    pub limit: Input<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct MessagesArgs {
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[schemars(!default, !skip_serializing_if)]
    /// Agent ID whose sent and received messages to read.
    pub target: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[schemars(range(min = 0))]
    /// Read messages older than this sequence; use `next_before` for older pages.
    pub before: Input<u64>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[schemars(range(min = 1))]
    /// Maximum messages, 1..200; default 100.
    pub limit: Input<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct BatchArgs {
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    /// Optional reply collection; an empty object collects without a deadline.
    pub join: Input<super::joins::JoinOptions>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[schemars(!default, !skip_serializing_if)]
    #[schemars(length(min = 1, max = crate::runtime::limits::BATCH_OPERATION_LIMIT))]
    /// 1..64 spawn/send/terminate operations, each with its own stable `request_id`.
    pub operations: Input<Vec<BatchItem>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct SendArgs {
    /// Up to 10 ordered absolute source paths; private `Hive` state is refused.
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[schemars(length(max = crate::runtime::attachments::MAX_FILES))]
    pub attachments: Input<Vec<std::path::PathBuf>>,

    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[schemars(range(min = 1, max = crate::runtime::limits::MAX_DELAY))]
    /// Schedule next-turn input 1..604800 seconds from now; omit for immediate acceptance.
    pub delay_seconds: Input<u64>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[schemars(!default, !skip_serializing_if)]
    #[doc = " Recipient agent ID, or @parent / @child for hosted callers (exactly one retained \
        direct child)."]
    pub target: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[schemars(!default, !skip_serializing_if)]
    /// Message text, 1..65536 UTF-8 bytes.
    pub body: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[schemars(with = "crate::runtime::domain::DeliveryMode")]
    #[doc = " next-turn (default), steer at a supported boundary, or redirect to interrupt and \
        queue first."]
    pub mode: Input<crate::runtime::domain::DeliveryMode>,
}

impl SendArgs {
    pub(crate) fn delivery_mode(&self) -> Result<Option<crate::runtime::domain::DeliveryMode>> {
        match &self.mode {
            Input::Invalid(raw) if raw.is_string() => {
                Err(crate::error::Error::unsupported_operation(
                    "Delivery mode must be next-turn, steer, or redirect.",
                ))
            }
            _ => Ok(self
                .mode
                .optional(Op::Send, "/mode", "next-turn, steer or redirect")?
                .copied()),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct SetOptionsArgs {
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[schemars(!default, !skip_serializing_if)]
    /// Agent whose desired settings will change at its next turn boundary.
    pub target: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    /// Partial catalog map. Null resets a key to the frozen spawn value.
    pub options: Input<crate::options::OptionMap>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    /// Native overlay text, snapshotted by `Hive`; not a filesystem path.
    pub config: Input<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct CompleteArgs {
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub target: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[schemars(!default, !skip_serializing_if)]
    pub body: Input<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct TargetArgs {
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[schemars(!default, !skip_serializing_if)]
    #[doc = " Agent ID to terminate permanently, or @parent / @child for hosted callers \
        (exactly one retained direct child)."]
    pub target: Input<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MessageResultArgs {
    #[serde(default)]
    pub message_id: Input<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct SetAuthArgs {
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[schemars(!default, !skip_serializing_if)]
    /// Agent ID whose future login to change.
    pub target: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[schemars(!default, !skip_serializing_if)]
    /// Subscription login profile or fallback group for the same provider.
    pub auth: Input<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct SetNameArgs {
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[schemars(!default, !skip_serializing_if)]
    /// Agent ID whose display name changes at once.
    pub target: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[doc = " New display name, 1..64 UTF-8 bytes without control characters or surrounding \
        spaces; not unique, and never selects an agent. Omit it with clear."]
    pub name: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    /// true removes the display name; omit it with name.
    pub clear: Input<bool>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AuthArgs {
    #[serde(default)]
    pub auth: Input<String>,
}
/// Every login's usage, or only `auth`'s.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UsageArgs {
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub auth: Input<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeySourceArgs {
    pub auth: String,
    pub profile: crate::config::Profile,
    pub register: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct OptionsCatalogArgs {
    /// Optional provider filter; omit to list both provider catalogs.
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub provider: Input<crate::config::Provider>,
    /// Optional resolved model token for recorded capability filtering.
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub model: Input<String>,
}
