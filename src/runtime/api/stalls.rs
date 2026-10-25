//! Read-only group queries; cursors identify the last emitted group in a snapshot.
use super::{AgentsArgs, InputContext as Op};
use crate::error::Result;
use hive_input::Code;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const STALLED_PREVIEW: usize = 20;
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StalledCursor {
    /// Unix second from the previous page cursor, 0..9223372036854775807.
    pub idle_since: u64,
    /// Root agent ID from the previous page cursor, 1..128 UTF-8 bytes.
    pub root_id: String,
}
impl<'de> Deserialize<'de> for StalledCursor {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            idle_since: u64,
            root_id: String,
        }
        let Fields {
            idle_since,
            root_id,
        } = crate::runtime::domain::decode::object(deserializer)?;
        Ok(Self {
            idle_since,
            root_id,
        })
    }
}
impl StalledCursor {
    pub(crate) fn validate(&self) -> Result<()> {
        hive_input::validate::text_bytes(&self.root_id, crate::runtime::limits::CURSOR_ID_BYTES)
            .map_err(|rule| {
                Op::Agents.error(rule.code(), "/stalled_after/root_id", rule.expected())
            })?;
        if self.idle_since > i64::MAX as u64 {
            return Err(Op::Agents.error(
                Code::OutOfRange,
                "/stalled_after/idle_since",
                "an integer from 0 to 9223372036854775807",
            ));
        }
        Ok(())
    }
}
impl AgentsArgs {
    pub(crate) fn stalled_requested(&self) -> bool {
        !self.stalled_seconds.is_missing() || !self.stalled_after.is_missing()
    }
    pub(crate) fn validate_stalled(&self) -> Result<u64> {
        let seconds = self
            .stalled_seconds
            .optional(
                Op::Agents,
                "/stalled_seconds",
                "an integer from 1 to 9223372036854775807",
            )?
            .copied()
            .ok_or_else(|| {
                Op::Agents.error(
                    Code::MissingArgument,
                    "/stalled_seconds",
                    "positive integer seconds when stalled_after is supplied",
                )
            })?;
        if seconds == 0 || seconds > i64::MAX as u64 {
            return Err(Op::Agents.error(
                Code::OutOfRange,
                "/stalled_seconds",
                "an integer from 1 to 9223372036854775807",
            ));
        }
        for (field, supplied) in [
            ("/after", !self.after.is_missing()),
            ("/parent", !self.parent.is_missing()),
            ("/auth", !self.auth.is_missing()),
        ] {
            if supplied {
                return Err(Op::Agents.error(
                    Code::ConflictingArguments,
                    field,
                    "omit this filter when stalled_seconds is supplied",
                ));
            }
        }
        if let Some(cursor) = self.stalled_after.optional(
            super::InputContext::Agents,
            "/stalled_after",
            "an object with idle_since and root_id",
        )? {
            cursor.validate()?;
        }
        super::pagination::rows(super::pagination::number(
            &self.limit,
            crate::runtime::limits::DEFAULT_HISTORY_PAGE_ROWS,
            "/limit",
        )?)?;
        Ok(seconds)
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct StalledGroup {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_for_build: Option<crate::runtime::api::RequiredBuild>,

    pub root_id: String,
    pub role: Option<String>,
    /// The root's display name.
    pub name: Option<String>,
    pub idle_since: u64,
    pub open_count: usize,
    pub open_preview: Vec<String>,
    pub any_paused: bool,
}
impl StalledGroup {
    pub(crate) fn cursor(&self) -> StalledCursor {
        StalledCursor {
            idle_since: self.idle_since,
            root_id: self.root_id.clone(),
        }
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct StalledPage {
    pub groups: Vec<StalledGroup>,
    pub observed_at: u64,
    pub stalled_seconds: u64,
    pub next_cursor: Option<StalledCursor>,
}
