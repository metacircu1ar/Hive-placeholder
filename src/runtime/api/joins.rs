//! Durable fan-in contracts shared by tools, control readers and CLI observers.
use super::{Input, OutputSelection};
use crate::error::Result;
use hive_input::Code;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub(crate) const MEMBER_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct JoinOptions {
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    /// Positive seconds; current Unix time plus this value must fit 9223372036854775807.
    #[schemars(range(min = 1))]
    pub deadline_seconds: Input<u64>,
}
impl<'de> Deserialize<'de> for JoinOptions {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            #[serde(default)]
            deadline_seconds: Input<u64>,
        }
        let fields: Fields = crate::runtime::domain::decode::object(d)?;
        Ok(Self {
            deadline_seconds: fields.deadline_seconds,
        })
    }
}
impl JoinOptions {
    pub fn deadline(&self, now: u64) -> Result<Option<u64>> {
        match self.deadline_seconds {
            Input::Missing => Ok(None),
            Input::Valid(seconds) if seconds > 0 => now
                .checked_add(seconds)
                .filter(|v| i64::try_from(*v).is_ok())
                .map(Some)
                .ok_or_else(|| {
                    super::batch::error(
                        Code::OutOfRange,
                        "/join/deadline_seconds",
                        "positive \
                    seconds whose sum with current Unix time fits 9223372036854775807",
                    )
                }),
            _ => Err(super::batch::error(
                Code::InvalidDuration,
                "/join/deadline_seconds",
                "positive \
                integer seconds whose sum with current Unix time fits 9223372036854775807",
            )),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct JoinAccepted {
    pub join_id: String,
    pub members: Vec<String>,
}
crate::runtime::domain::domain_enum!(JoinState {
    Collecting => "collecting",
    DeadlinePassed => "deadline_passed",
    Completed => "completed",
    Cancelled => "cancelled",
});
crate::runtime::domain::domain_enum!(JoinOutcome {
    Pending => "pending",
    Completed => "completed",
    Failed => "failed",
    Interrupted => "interrupted",
    Undeliverable => "undeliverable",
    Uncertain => "uncertain",
});
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct JoinOutput {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_target: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct JoinMember {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_for_build: Option<crate::runtime::api::RequiredBuild>,

    pub position: usize,
    pub message_id: String,
    pub target: String,
    pub outcome: JoinOutcome,
    #[serde(deserialize_with = "crate::runtime::domain::decode::optional_object")]
    pub output: Option<JoinOutput>,
    #[serde(deserialize_with = "crate::runtime::domain::decode::string_enum")]
    pub output_selection: OutputSelection,
    pub output_partial: bool,
    #[serde(deserialize_with = "crate::runtime::domain::decode::optional_object")]
    pub task_completed: Option<JoinOutput>,
    pub delivered: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct JoinSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_for_build: Option<crate::runtime::api::RequiredBuild>,

    pub join_id: String,
    pub requester: String,
    pub request_id: String,
    pub created_at: u64,
    pub deadline: Option<u64>,
    pub state: JoinState,
    #[serde(deserialize_with = "super::responses::objects")]
    pub members: Vec<JoinMember>,
}
impl JoinSnapshot {
    pub fn closed(&self) -> bool {
        matches!(self.state, JoinState::Completed | JoinState::Cancelled)
    }
}
