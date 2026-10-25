//! Daemon health, capacity and provider feature responses.
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct PingResponse {
    pub binary_sha256: String,
    pub commit: Option<&'static str>,
    pub protocols: hive_input::compatibility::ProtocolRange,
    pub agent_catalog: u32,
    pub state_format: u32,
    pub provider_compatibility: &'static [crate::provider_support::Registry],
    pub provider_selection: crate::runtime::ProviderSelection,
    pub configuration: Option<crate::config::ConfigurationIdentity>,
    pub deprecations: Vec<crate::config::deprecations::Notice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub managed_service: Option<&'static str>,
    pub product_version: &'static str,
    pub compiled_features: &'static [&'static str],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comb_backlog: Option<CombBacklog>,
    pub status: &'static str,
    pub pid: u32,
    pub storage: StorageDiagnostics,
    pub resident_actors: usize,
    pub profile_leases: usize,
    pub capabilities: Capabilities,
    pub writer_observation: WriterObservation,
}
#[derive(Serialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub(crate) enum WriterObservation {
    Committed {
        runtime_id: String,
        sequence: u64,
        captured_unix_ms: u64,
        age_ns: u64,
    },
}
#[derive(Serialize)]
pub(crate) struct Capabilities {
    pub providers: [ProviderCapabilities; 2],
    pub approvals: &'static str,
}
#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)] // Independent advertised provider features.
pub(crate) struct ProviderCapabilities {
    pub provider: &'static str,
    pub subscription: bool,
    pub next_turn: bool,
    pub redirect: bool,
    pub steering_policy: SteeringPolicy,
    pub read_only_shell: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SteeringPolicy {
    NativeProbe,
    VerifiedCanary,
}
#[derive(Serialize)]
pub(crate) struct StorageDiagnostics {
    pub sqlite_allocated_bytes: i64,
    pub sqlite_cache_bytes: i32,
    pub pending_wake_ids: usize,
    pub ready: bool,
}
#[derive(Serialize)]
pub(crate) struct RuntimeResources<'a> {
    pub provider_versions: Vec<&'a crate::provider_support::Observation>,
    pub active_turns: usize,
    pub reserved_shards: usize,
    pub recovered_provider_reservations: usize,
    pub unreaped_provider_reservations: usize,
    pub recovered_unresolved_activity_profiles: usize,
    pub resident_session_slots: usize,
    pub attached_agents: usize,
    pub starting_shards: usize,
    pub warm_shards: usize,
    pub lingering_shards: usize,
    pub draining_shards: usize,
    pub limits: &'a crate::config::Runtime,
}

/// Decorations added after a control request leaves the committed reader/writer.
#[derive(Default, Serialize)]
pub(crate) struct ControlReply {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleanup_pending: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_resources: Option<serde_json::Value>,
}

impl ControlReply {
    #[expect(
        clippy::expect_used,
        reason = "Control replies are constructed from object-shaped response records."
    )]
    pub fn decorate(self, mut response: serde_json::Value) -> serde_json::Value {
        if self.request_id.is_none()
            && self.cleanup_pending.is_none()
            && self.runtime_resources.is_none()
        {
            return response;
        }
        // Retain the owned response tree: serializing the flattened response would
        // copy every payload even when the request has no decorations.
        let serde_json::Value::Object(fields) = super::value(&self) else {
            unreachable!("control decorations serialize as an object");
        };
        response
            .as_object_mut()
            .expect("control replies are objects")
            .extend(fields);
        response
    }
}

#[derive(Clone, Copy, Serialize)]
pub(crate) struct CombBacklog {
    pub collectors: u64,
    pub rows: u64,
    pub delivery_supported: bool,
}
