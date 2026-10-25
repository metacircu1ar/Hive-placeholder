//! Grouped agent state with one strict flat storage record and stable public projections.
use super::{
    AgentState, Control, OptionsSelection, RecoveryFence, ReservedTurn, TaskOutcome, decode,
    is_false,
};
use crate::config::{Lifetime, Profile, Sandbox};
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
mod selections;

// Declare ordinary fields once; the record validates the exact flat storage shape,
// while runtime code can only construct complete grouped identities and states.
macro_rules! agent_record {
    ($($(#[$attribute:meta])* $field:ident: $kind:ty $(=> $skip:path)?),+ $(,)?) => {
        #[derive(Clone, Debug)]
        // Independent lifecycle, billing and delivery facts.
        pub struct Agent<B = crate::config::CombBinding,
             M = crate::config::McpServers,
             O = crate::options::AgentOptions> {
            pub comb: Option<B>,
            pub mcp_servers: M,
            $(pub $field: $kind,)+
            pub auth_cutoff: Option<AuthCutoff>,
            pub provider_process: Option<ProviderProcess>,
            pub steering: SteeringCapability,
        }
        #[derive(Deserialize)]
#[serde(deny_unknown_fields, bound(deserialize = "B: serde::de::DeserializeOwned, M: \
    Deserialize<'de>, O: Deserialize<'de>"))]
        // Flat storage projection of independent agent facts.
        struct Record<B, M, O> {
            #[serde(deserialize_with = "decode::optional_object")]
            comb: Option<B>,
            mcp_servers: M,
            $($(#[$attribute])* $field: agent_record!(@stored_type $field, $kind),)+
            #[serde(deserialize_with = "serde::Deserialize::deserialize")]
            auth_cutoff_turn: Option<String>,
            #[serde(deserialize_with = "serde::Deserialize::deserialize")]
            auth_cutoff_message: Option<String>,
            #[serde(deserialize_with = "serde::Deserialize::deserialize")]
            provider_id: Option<String>,
            #[serde(deserialize_with = "serde::Deserialize::deserialize")]
            provider_pid: Option<u32>,
            #[serde(deserialize_with = "serde::Deserialize::deserialize")]
            provider_spawn_before: Option<u64>,
            #[serde(deserialize_with = "serde::Deserialize::deserialize")]
            provider_spawn_after: Option<u64>,
            #[serde(default)]
            steering_supported: bool,
            #[serde(default)]
            steering_disabled_reason: Option<String>,
            #[serde(default)]
            steering_proof_error_code: Option<String>,
        }
        impl<'de, B: serde::de::DeserializeOwned,
             M: Deserialize<'de>,
             O: Deserialize<'de> + crate::options::SnapshotContext> Deserialize<'de> for Agent<B,
             M,
             O> {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let record: Record<B, M, O> = decode::object(deserializer)?;
                let selections = selections::Records {
                    reserved_turn: record.reserved_turn,
                    session_selection: record.session_selection,
                    last_accepted_selection: record.last_accepted_selection,
                    turn_selection: record.turn_selection,
                }.hydrate(&record.agent_options).map_err(serde::de::Error::custom)?;
                let auth_cutoff = match (record.auth_cutoff_turn, record.auth_cutoff_message) {
                    (None, None) => None,
                    (Some(turn), None) => Some(AuthCutoff::Observed { turn }),
                    (Some(turn), Some(message)) => Some(AuthCutoff::Bound { turn, message }),
                    (None, Some(_)) => {
                        return Err(serde::de::Error::custom("Cutoff input has no observed turn."));
                    }
                };
                let provider_process = match (
                    record.provider_id,
                    record.provider_pid,
                    record.provider_spawn_before,
                    record.provider_spawn_after,
                ) {
                    (None, None, None, None) => None,
                    (Some(id), None, None, None) => Some(ProviderProcess::Reserved { id }),
                    (Some(id), Some(pid), Some(before), Some(after)) => {
                        Some(ProviderProcess::Started {
                            id,
                            pid,
                            before,
                            after,
                        })
                    }
                    _ => {
                        return Err(serde::de::Error::custom(
                            "Incomplete provider process identity.",
                        ));
                    }
                };
                let steering = if record.steering_supported {
                    if record.steering_disabled_reason.is_some()
                        || record.steering_proof_error_code.is_some()
                    {
                        return Err(serde::de::Error::custom(
                            "Enabled steering has disabled diagnostics.",
                        ));
                    }
                    SteeringCapability::Supported
                } else {
                    SteeringCapability::Disabled {
                        reason: record.steering_disabled_reason,
                        error_code: record.steering_proof_error_code,
                    }
                };
                Ok(Self {
                    comb: record.comb,
                    mcp_servers: record.mcp_servers,
                    $($field: agent_record!(@hydrated record, selections, $field),)+
                    auth_cutoff,
                    provider_process,
                    steering,
                })
            }
        }
        impl<B: Serialize, M: Serialize,
             O: Serialize + crate::options::SnapshotContext> Serialize for Agent<B,
             M,
             O> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut map = serializer.serialize_map(None)?;
                map.serialize_entry("comb", &self.comb)?;
                map.serialize_entry("mcp_servers", &self.mcp_servers)?;
                $(if agent_record!(@include self.$field $(, $skip)?) {
                    agent_record!(@serialize map, self, $field);
                })+
                map.serialize_entry(
                    "auth_cutoff_turn",
                    &self.auth_cutoff.as_ref().map(AuthCutoff::turn),
                )?;
                map.serialize_entry(
                    "auth_cutoff_message",
                    &self.auth_cutoff.as_ref().and_then(AuthCutoff::message),
                )?;
                map.serialize_entry("provider_id",
                     &self.provider_process.as_ref().map(ProviderProcess::id))?;
                map.serialize_entry(
                    "provider_pid",
                    &self.provider_process.as_ref().and_then(ProviderProcess::pid),
                )?;
                map.serialize_entry(
                    "provider_spawn_before",
                    &self.provider_process.as_ref().and_then(ProviderProcess::before),
                )?;
                map.serialize_entry(
                    "provider_spawn_after",
                    &self.provider_process.as_ref().and_then(ProviderProcess::after),
                )?;
                if self.steering.supported() {
                    map.serialize_entry("steering_supported", &true)?;
                }
                if let Some(reason) = self.steering.reason() {
                    map.serialize_entry("steering_disabled_reason", reason)?;
                }
                if let Some(code) = self.steering.error_code() {
                    map.serialize_entry("steering_proof_error_code", code)?;
                }
                map.end()
            }
        }
        impl Agent {
            pub(crate) fn public(self) -> Agent<CombCapability,
                 Vec<String>,
                 crate::options::OptionsView> {
                Agent {
                    comb: self.comb.map(|binding| CombCapability { reviewer: binding.reviewer }),
                    mcp_servers: self.mcp_servers.into_keys().collect(),
                    $($field: agent_record!(@public self, $field),)+
                    auth_cutoff: self.auth_cutoff,
                    provider_process: self.provider_process,
                    steering: self.steering,
                }
            }
        }
    };
    (@stored_type reserved_turn, $kind:ty) => { Option<selections::Reservation> };
    (@stored_type session_selection, $kind:ty) => { Option<selections::Selection> };
    (@stored_type last_accepted_selection, $kind:ty) => { Option<selections::Selection> };
    (@stored_type turn_selection, $kind:ty) => { Option<selections::Selection> };
    (@stored_type $field:ident, $kind:ty) => { $kind };
    (@hydrated $record:ident, $selections:ident, reserved_turn) => { $selections.reserved_turn };
    (@hydrated $record:ident,
         $selections:ident,
         session_selection) => { $selections.session_selection };
    (@hydrated $record:ident,
         $selections:ident,
         last_accepted_selection) => { $selections.last_accepted_selection };
    (@hydrated $record:ident, $selections:ident, turn_selection) => { $selections.turn_selection };
    (@hydrated $record:ident, $selections:ident, $field:ident) => { $record.$field };
    (@serialize $map:ident, $agent:ident, reserved_turn) => {
        $map.serialize_entry("reserved_turn",
             &selections::reservation($agent.reserved_turn.as_ref(),
             $agent.agent_options.baseline_snapshot()))?;
    };
    (@serialize $map:ident, $agent:ident, session_selection) => {
        $map.serialize_entry("session_selection",
             &selections::selection($agent.session_selection.as_ref(),
             $agent.agent_options.baseline_snapshot()))?;
    };
    (@serialize $map:ident, $agent:ident, last_accepted_selection) => {
        $map.serialize_entry("last_accepted_selection",
             &selections::selection($agent.last_accepted_selection.as_ref(),
             $agent.agent_options.baseline_snapshot()))?;
    };
    (@serialize $map:ident, $agent:ident, turn_selection) => {
        $map.serialize_entry("turn_selection",
             &selections::selection($agent.turn_selection.as_ref(),
             $agent.agent_options.baseline_snapshot()))?;
    };
    (@serialize $map:ident, $agent:ident, $field:ident) => {
        $map.serialize_entry(stringify!($field), &$agent.$field)?;
    };
    (@public $agent:ident, agent_options) => { $agent.agent_options.public() };
    (@public $agent:ident, reserved_turn) => {
        $agent.reserved_turn.map(crate::runtime::domain::ReservedTurn::public)
    };
    (@public $agent:ident, session_selection) => {
        $agent.session_selection.map(crate::runtime::domain::OptionsSelection::public)
    };
    (@public $agent:ident, last_accepted_selection) => {
        $agent.last_accepted_selection.map(crate::runtime::domain::OptionsSelection::public)
    };
    (@public $agent:ident, turn_selection) => {
        $agent.turn_selection.map(crate::runtime::domain::OptionsSelection::public)
    };
    (@public $agent:ident, $field:ident) => { $agent.$field };
    (@include $field:expr, $skip:path) => {
        !$skip(&$field)
    };
    (@include $field:expr) => {
        true
    };
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CombCapability {
    pub reviewer: bool,
}

agent_record! {
    id: String,
    tool_catalog: crate::compatibility::AgentCatalog,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    role: Option<String>,
    /// A display label; never an identifier, and not unique.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String> => Option::is_none,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    parent_id: Option<String>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    branched_from_agent_id: Option<String>,
    /// Storage owns admission gates independently of ordinary agent saves.
    branch_hold: u64,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    branch_phase: Option<crate::runtime::branching::BranchPhase>,
    #[serde(deserialize_with = "decode::optional_object")]
    last_finished_turn: Option<crate::runtime::branching::FinishedTurn>,
    #[serde(deserialize_with = "decode::optional_object")]
    last_attempted_turn: Option<crate::runtime::branching::AttemptedTurn>,
    auth: String,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    auth_group: Option<String>,
    #[serde(deserialize_with = "decode::optional_object")]
    auth_decision: Option<crate::runtime::auth_fallback::AuthDecision>,
    api_fallback: bool,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    waiting_reason: Option<crate::runtime::auth_fallback::WaitingReason>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    quota_wait_until: Option<u64>,
    #[serde(deserialize_with = "decode::optional_object")]
    auth_switch_pending: Option<crate::runtime::auth_fallback::SwitchIntent>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    handoff: Option<String>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    turn_auth_revision: Option<u64>,
    #[serde(deserialize_with = "decode::optional_object")]
    execution_key: Option<crate::state::KeyAuthority>,
    #[serde(deserialize_with = "decode::optional_object")]
    model_key: Option<crate::state::KeyAuthority>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    auth_probe_id: Option<String>,
    auth_checked_revision: u64,
    auth_generation: uuid::Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recovery_fence: Option<RecoveryFence> => Option::is_none,
    conversation_clean: bool,
    #[serde(deserialize_with = "decode::profile")]
    profile: Profile,
    model: String,
    effort: String,
    options_revision: u64,
    #[serde(deserialize_with = "decode::object")]
    agent_options: O,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    applied_options_revision: Option<u64>,
    #[serde(deserialize_with = "decode::optional_object")]
    reserved_turn: Option<ReservedTurn>,
    #[serde(deserialize_with = "decode::optional_object")]
    session_selection: Option<OptionsSelection>,
    #[serde(deserialize_with = "decode::optional_object")]
    last_accepted_selection: Option<OptionsSelection>,
    #[serde(deserialize_with = "decode::optional_object")]
    turn_selection: Option<OptionsSelection>,
    #[serde(default, skip_serializing_if = "is_false")]
    redirect_pending: bool => is_false,
    cwd: PathBuf,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    instructions: String => String::is_empty,
    #[serde(deserialize_with = "decode::string_enum")]
    lifetime: Lifetime,
    #[serde(deserialize_with = "decode::string_enum")]
    sandbox: Sandbox,
    state: AgentState,
    task_outcome: TaskOutcome,
    paused: bool,
    expired: bool,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    native_input_attempted: bool => is_false,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    execution_id: Option<String>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    turn_id: Option<String>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    control: Option<Control>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    completion_pending: Option<String>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    result: Option<String>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    error: Option<String>,
    created_at: u64,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    ended_at: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderProcess {
    Reserved {
        id: String,
    },
    Started {
        id: String,
        pid: u32,
        before: u64,
        after: u64,
    },
}
impl ProviderProcess {
    pub fn id(&self) -> &str {
        match self {
            Self::Reserved { id } | Self::Started { id, .. } => id,
        }
    }
    pub fn pid(&self) -> Option<u32> {
        match self {
            Self::Started { pid, .. } => Some(*pid),
            Self::Reserved { .. } => None,
        }
    }
    pub fn before(&self) -> Option<u64> {
        match self {
            Self::Started { before, .. } => Some(*before),
            Self::Reserved { .. } => None,
        }
    }
    pub fn after(&self) -> Option<u64> {
        match self {
            Self::Started { after, .. } => Some(*after),
            Self::Reserved { .. } => None,
        }
    }
}

/// Quota evidence may arrive before the input acknowledgement binds its message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuthCutoff {
    Observed { turn: String },
    Bound { turn: String, message: String },
}
impl AuthCutoff {
    pub fn turn(&self) -> &str {
        match self {
            Self::Observed { turn } | Self::Bound { turn, .. } => turn,
        }
    }
    pub fn message(&self) -> Option<&str> {
        match self {
            Self::Observed { .. } => None,
            Self::Bound { message, .. } => Some(message),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SteeringCapability {
    Supported,
    Disabled {
        reason: Option<String>,
        error_code: Option<String>,
    },
}
impl Default for SteeringCapability {
    fn default() -> Self {
        Self::Disabled {
            reason: None,
            error_code: None,
        }
    }
}
impl SteeringCapability {
    pub fn supported(&self) -> bool {
        matches!(self, Self::Supported)
    }
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Supported => None,
            Self::Disabled { reason, .. } => reason.as_deref(),
        }
    }
    pub fn error_code(&self) -> Option<&str> {
        match self {
            Self::Supported => None,
            Self::Disabled { error_code, .. } => error_code.as_deref(),
        }
    }
}

impl Agent {
    pub fn provider_id(&self) -> Option<&str> {
        self.provider_process.as_ref().map(ProviderProcess::id)
    }
    pub fn clear_turn(&mut self) {
        self.turn_id = None;
        self.turn_selection = None;
    }
    pub fn clear_cutoff(&mut self) {
        self.auth_cutoff = None;
    }
    pub fn disable_steering(&mut self) {
        if self.steering.supported() {
            self.steering = SteeringCapability::default();
        }
    }
    pub fn clear_auth_wait(&mut self, revision: u64) {
        self.waiting_reason = None;
        self.quota_wait_until = None;
        self.auth_switch_pending = None;
        self.auth_checked_revision = revision;
    }
    pub fn reset_conversation(&mut self, binding: &crate::state::Binding) {
        self.auth_generation = binding.id;
        self.profile = binding.profile.clone();
        self.thread_id = (self.profile.provider == crate::config::Provider::Claude)
            .then(|| uuid::Uuid::new_v4().to_string());
        self.native_input_attempted = false;
        self.conversation_clean = true;
        self.execution_id = None;
        self.provider_process = None;
        self.session_selection = None;
        self.last_accepted_selection = None;
        self.clear_turn();
        self.last_finished_turn = None;
        self.last_attempted_turn = None;
        self.turn_auth_revision = None;
        self.auth_probe_id = None;
        self.clear_cutoff();
        self.disable_steering();
    }
}
