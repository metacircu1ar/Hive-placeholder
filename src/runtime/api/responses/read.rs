//! Strict daemon-response decisions with borrowed raw output payloads.
use super::{AgentResponse, EventsPage};
use crate::runtime::client::envelope::{self, Policy, invalid};
use crate::{
    error::Result,
    runtime::domain::{Agent, AgentState, EventKind, TaskOutcome, decode},
};
use serde::{Deserialize, de::IgnoredAny};
use serde_json::Value;

pub(crate) fn objects<'de, D, T>(deserializer: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    Vec::<Value>::deserialize(deserializer)?
        .iter()
        .map(|value| decode::object(value).map_err(serde::de::Error::custom))
        .collect()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusExtras {
    held_for_build: Option<super::RequiredBuild>,
    #[serde(default)]
    open_joins: Vec<String>,
    #[serde(default, deserialize_with = "decode::optional_object")]
    branch: Option<crate::runtime::branching::Intent>,
    #[serde(default)]
    branch_dependents: Vec<String>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    idle_since: Option<u64>,
    #[serde(deserialize_with = "decode::string_enum")]
    provider: crate::config::Provider,
    #[serde(deserialize_with = "decode::string_enum")]
    billing: crate::config::Method,
    waiting_on: Option<String>,
    approval_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EventFields {
    seq: i64,
    agent_id: String,
    #[serde(
        rename = "execution_id",
        deserialize_with = "serde::Deserialize::deserialize"
    )]
    _execution_id: Option<String>,
    #[serde(rename = "timestamp")]
    _timestamp: i64,
    #[serde(rename = "type")]
    kind: EventKind,
    #[serde(rename = "data")]
    _data: IgnoredAny,
}

pub(crate) struct DaemonReply<'a> {
    raw: &'a Value,
    error: Option<envelope::ErrorEnvelope>,
}
impl<'a> DaemonReply<'a> {
    pub fn new(raw: &'a Value) -> Result<Self> {
        let error = envelope::error(raw, Policy::Command)?;
        Ok(Self { raw, error })
    }
    pub fn has_error(&self) -> bool {
        self.error.is_some()
    }
    pub fn error_code(&self) -> Option<crate::error::ErrorCode> {
        self.error.as_ref().map(|envelope| envelope.error.code())
    }
    pub fn error_message(&self) -> Option<&str> {
        self.error.as_ref().map(|envelope| envelope.error.message())
    }
    pub fn input_error(&self) -> Option<(&hive_input::InputError, Option<&str>)> {
        let envelope = self.error.as_ref()?;
        Some((envelope.error.input()?, envelope.request_id.as_deref()))
    }
    pub fn agent(&self) -> Result<AgentStatus<'a>> {
        let _: AgentResponse<IgnoredAny> =
            decode::object(self.raw).map_err(|cause| invalid().with_source(cause))?;
        let mut fields = self.raw["agent"].as_object().ok_or_else(invalid)?.clone();
        let mut extras = serde_json::Map::new();
        for key in [
            "held_for_build",
            "open_joins",
            "branch",
            "branch_dependents",
            "idle_since",
            "provider",
            "billing",
            "waiting_on",
            "approval_id",
        ] {
            if let Some(value) = fields.remove(key) {
                extras.insert(key.into(), value);
            }
        }
        let extras: StatusExtras = serde_json::from_value(Value::Object(extras))
            .map_err(|cause| invalid().with_source(cause))?;
        let agent: Agent<
            crate::runtime::domain::CombCapability,
            Vec<String>,
            crate::options::OptionsView,
        > = serde_json::from_value(Value::Object(fields))
            .map_err(|cause| invalid().with_source(cause))?;
        if extras.provider != agent.profile.provider
            || extras.billing != agent.profile.method
            || extras.waiting_on.as_deref()
                != extras
                    .approval_id
                    .as_ref()
                    .map(|_| crate::runtime::hub::Caller::Human.name())
        {
            return Err(invalid());
        }
        // These fields are validated above even though waiting decisions do not inspect them.
        let _ = (
            extras.open_joins,
            extras.branch,
            extras.branch_dependents,
            extras.idle_since,
        );
        Ok(AgentStatus {
            held_for_build: extras.held_for_build,
            raw: &self.raw["agent"],
            fields: agent,
        })
    }
    pub fn events(&self) -> Result<EventPage<'a>> {
        let fields: EventsPage<IgnoredAny> =
            decode::object(self.raw).map_err(|cause| invalid().with_source(cause))?;
        let events = self.raw["events"]
            .as_array()
            .ok_or_else(invalid)?
            .iter()
            .map(|event| {
                decode::object::<_, EventFields>(event)
                    .map_err(|cause| invalid().with_source(cause))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(EventPage {
            raw: self.raw,
            fields,
            events,
        })
    }
}

pub(crate) struct AgentStatus<'a> {
    held_for_build: Option<super::RequiredBuild>,
    raw: &'a Value,
    fields: Agent<crate::runtime::domain::CombCapability, Vec<String>, crate::options::OptionsView>,
}
impl<'a> AgentStatus<'a> {
    pub fn held(&self) -> bool {
        self.held_for_build.is_some()
    }
    pub fn raw(&self) -> &'a Value {
        self.raw
    }
    pub fn outcome(&self) -> TaskOutcome {
        self.fields.task_outcome
    }
    pub fn expired(&self) -> bool {
        self.fields.expired
    }
    pub fn state(&self) -> AgentState {
        self.fields.state
    }
}

pub(crate) struct EventPage<'a> {
    raw: &'a Value,
    fields: EventsPage<IgnoredAny>,
    events: Vec<EventFields>,
}
impl<'a> EventPage<'a> {
    pub fn history_expired(&self) -> bool {
        self.fields.history_expired
    }
    pub fn cursor(&self) -> u64 {
        self.fields.cursor
    }
    pub fn pruned_through(&self) -> &'a Value {
        &self.raw["pruned_through"]
    }
    pub fn first_turn_finished(&self) -> Option<&'a Value> {
        self.events
            .iter()
            .position(|event| event.kind == EventKind::TurnFinished)
            .map(|index| &self.raw["events"][index])
    }
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
    pub fn validate_follow(&self, target: &str, after: u64, limit: u64) -> Result<()> {
        if self.fields.history_expired != (after < self.fields.pruned_through)
            || self.events.len() as u64 > limit
        {
            return Err(invalid());
        }
        let mut cursor = after.max(self.fields.pruned_through);
        for event in &self.events {
            let sequence =
                u64::try_from(event.seq).map_err(|cause| invalid().with_source(cause))?;
            // Sequences are global: gaps between this agent's events are valid.
            if event.agent_id != target || sequence <= cursor {
                return Err(invalid());
            }
            cursor = sequence;
        }
        if self.fields.cursor != cursor {
            return Err(invalid());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
