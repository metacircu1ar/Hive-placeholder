//! Runtime agents, states, and their stable wire and storage representations.

pub(crate) mod decode;

use serde::{Deserialize, Serialize};

mod agent;
pub use agent::{Agent, AuthCutoff, CombCapability, ProviderProcess, SteeringCapability};

/// A requested selection, captured independently of subsequent desired changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptionsSelection {
    #[serde(deserialize_with = "decode::object")]
    pub options: std::sync::Arc<crate::options::Snapshot>,
    pub model: String,
    pub effort: String,
    pub revision: u64,
}
impl OptionsSelection {
    pub fn public(mut self) -> Self {
        self.options = std::sync::Arc::new(self.options.as_ref().clone().public());
        self
    }
}

/// Reserving accepted input does not mean that native input was attempted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReservedTurn {
    pub message_id: String,
    #[serde(deserialize_with = "decode::object")]
    pub selection: OptionsSelection,
}
impl ReservedTurn {
    pub fn public(mut self) -> Self {
        self.selection = self.selection.public();
        self
    }
}
#[allow(clippy::trivially_copy_pass_by_ref)] // Serde's skip predicate takes a reference.
fn is_false(value: &bool) -> bool {
    !value
}
impl Agent {
    pub fn boundary_selection(&self) -> OptionsSelection {
        self.reserved_turn.as_ref().map_or_else(
            || self.desired_selection(),
            |reserved| reserved.selection.clone(),
        )
    }
    pub fn desired_selection(&self) -> OptionsSelection {
        let mut options = self.agent_options.desired_shared();
        if options.string("model") != Some(self.model.as_str())
            || options.string("effort") != Some(self.effort.as_str())
            || !options.sources.contains_key("model")
            || !options.sources.contains_key("effort")
        {
            let options = std::sync::Arc::make_mut(&mut options);
            options.values.insert(
                "model".into(),
                serde_json::Value::String(self.model.clone()),
            );
            options.values.insert(
                "effort".into(),
                serde_json::Value::String(self.effort.clone()),
            );
            for key in ["model", "effort"] {
                options
                    .sources
                    .entry(key.into())
                    .or_insert_with(|| crate::options::OptionSource::plain("spawn"));
            }
        }
        OptionsSelection {
            options,
            model: self.model.clone(),
            effort: self.effort.clone(),
            revision: self.options_revision,
        }
    }
    pub fn terminal(&self) -> bool {
        matches!(
            self.task_outcome,
            TaskOutcome::Completed | TaskOutcome::Cancelled
        ) || self.expired
    }
}
macro_rules! domain_enum {
    (
        $(#[$enum_attribute:meta])*
        $name:ident {
            $($(#[$attribute:meta])* $variant:ident => $value:literal),+ $(,)?
        }
    ) => {
        $(#[$enum_attribute])*
        #[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, schemars::JsonSchema)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($(#[$attribute])* #[serde(rename = $value)] $variant),+
        }

        impl $name {
            pub const VALUES: &'static [&'static str] = &[$($(#[$attribute])* $value),+];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $($(#[$attribute])* Self::$variant => $value),+
                }
            }

            pub fn parse(value: &str) -> Option<Self> {
                match value {
                    $($(#[$attribute])* $value => Some(Self::$variant)),+,
                    _ => None,
                }
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(
                deserializer: D,
            ) -> std::result::Result<Self, D::Error> {
                struct Literal;
                impl serde::de::Visitor<'_> for Literal {
                    type Value = $name;
                    fn expecting(
                        &self,
                        formatter: &mut std::fmt::Formatter<'_>,
                    ) -> std::fmt::Result {
                        formatter.write_str(concat!("a literal ", stringify!($name), " string"))
                    }
                    fn visit_str<E: serde::de::Error>(
                        self,
                        value: &str,
                    ) -> std::result::Result<$name, E> {
                        $name::parse(value).ok_or_else(|| E::unknown_variant(value, $name::VALUES))
                    }
                }
                deserializer.deserialize_str(Literal)
            }
        }

        impl rusqlite::ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
                Ok(rusqlite::types::ToSqlOutput::Borrowed(
                    rusqlite::types::ValueRef::Text(self.as_str().as_bytes()),
                ))
            }
        }

        impl rusqlite::types::FromSql for $name {
            fn column_result(
                value: rusqlite::types::ValueRef<'_>,
            ) -> rusqlite::types::FromSqlResult<Self> {
                Self::parse(value.as_str()?).ok_or_else(|| {
                    rusqlite::types::FromSqlError::Other(
                        concat!("Invalid ", stringify!($name), ".").into(),
                    )
                })
            }
        }
    };
}

pub(crate) use domain_enum;

// Open provider vocabularies classify only known strings. The original frame remains
// available for evidence and forwarding; unknown strings are never reconstructed.
macro_rules! wire_enum {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
        #[serde(from = "String")]
        pub(crate) enum $name {
            $(#[serde(rename = $wire)] $variant,)+
            #[serde(other)]
            Other,
        }
        impl $name {
            pub fn parse(value: &str) -> Self {
                match value {
                    $($wire => Self::$variant,)+
                    _ => Self::Other
                }
            }
        }
        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self::parse(&value)
            }
        }
    };
}
pub(crate) use wire_enum;

domain_enum!(AgentState {
    Starting => "starting",
    Idle => "idle",
    Running => "running",
    Stopping => "stopping",
    Stopped => "stopped",
    Failed => "failed",
});

domain_enum!(TaskOutcome {
    Open => "open",
    Completed => "completed",
    Cancelled => "cancelled",
});

domain_enum!(MessageState {
    Scheduled => "scheduled",
    Accepted => "accepted",
    Submitting => "submitting",
    Submitted => "submitted",
    Completed => "completed",
    Interrupted => "interrupted",
    Failed => "failed",
    Uncertain => "uncertain",
    Undeliverable => "undeliverable",
    StaleTurn => "stale_turn",
});

domain_enum!(PurgeBlockerReason {
    PendingNativeBranches => "pending_native_branches",
    AgentNotEnded => "agent_not_ended",
    AgentCleanupPending => "agent_cleanup_pending",
    AgentCleanupUnconfirmed => "agent_cleanup_unconfirmed",
    ActiveInput => "active_input",
    UnresolvedReservation => "unresolved_reservation",
});

domain_enum!(MessageKind {
    ApprovalAnswer => "approval_answer",
    Continuation => "continuation",
    JoinResult => "join_result"
});

/// A fence marker or the original diagnostic text from a failed cleanup.
/// Diagnostic text is an opaque projection; only the two markers authorize clearing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecoveryFence {
    ExecutionCleanupUncertain,
    BranchCreationUncertain,
    Failure(String),
}
impl Serialize for RecoveryFence {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(match self {
            Self::ExecutionCleanupUncertain => "execution_cleanup_uncertain",
            Self::BranchCreationUncertain => "branch_creation_uncertain",
            Self::Failure(message) => message,
        })
    }
}
impl<'de> Deserialize<'de> for RecoveryFence {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Ok(match text.as_str() {
            "execution_cleanup_uncertain" => Self::ExecutionCleanupUncertain,
            "branch_creation_uncertain" => Self::BranchCreationUncertain,
            _ => Self::Failure(text),
        })
    }
}

domain_enum!(#[derive(clap::ValueEnum)] DeliveryMode {
    NextTurn => "next-turn",
    Steer => "steer",
    Redirect => "redirect",
});

impl DeliveryMode {
    #[allow(clippy::trivially_copy_pass_by_ref)] // Serde's skip predicate takes a reference.
    pub fn is_next_turn(&self) -> bool {
        *self == Self::NextTurn
    }
}

domain_enum!(Control {
    Stop => "stop",
    Interrupt => "interrupt",
});

domain_enum!(RedirectState {
    Pending => "pending",
    Sent => "sent",
    Resolved => "resolved",
    TargetLost => "target_lost",
});

// Current persisted and public event kinds.
domain_enum!(EventKind {
    AuthSwitched => "auth_switched",
    AuthSwitchRequested => "auth_switch_requested",
    AuthSwitchDeferred => "auth_switch_deferred",
    MemberSkipped => "member_skipped",
    ContinuationQueued => "continuation_queued",
    ApprovalRequested => "approval_requested",
    ApprovalClosed => "approval_closed",
    AssistantMessage => "assistant_message",
    CompletionRequested => "completion_requested",
    ControlRequested => "control_requested",
    ExecutionFailed => "execution_failed",
    ExecutionStarting => "execution_starting",
    ExecutionStopped => "execution_stopped",
    LifetimeExpired => "lifetime_expired",
    MessageAccepted => "message_accepted",
    MessagePromoted => "message_promoted",
    MessageScheduleFailed => "message_schedule_failed",
    RedirectAccepted => "redirect_accepted",
    SteeringSubmitted => "steering_submitted",
    SteeringRejected => "steering_rejected",
    SteeringUncertain => "steering_uncertain",
    ModelRerouted => "model_rerouted",
    OptionsChanged => "options_changed",
    ProviderError => "provider_error",
    ProviderSessionReleased => "provider_session_released",
    ProviderShutdown => "provider_shutdown",
    ProviderStarted => "provider_started",
    SessionQuarantined => "session_quarantined",
    SessionReady => "session_ready",
    Spawned => "spawned",
    JoinRequested => "join_requested",
    JoinCompleted => "join_completed",
    JoinCancelled => "join_cancelled",
    JoinDeadlinePassed => "join_deadline_passed",
    BranchRequested => "branch_requested",
    BranchReady => "branch_ready",
    BranchFailed => "branch_failed",
    BranchContextDropped => "branch_context_dropped",
    TaskCompleted => "task_completed",
    TerminationRequested => "termination_requested",
    TerminalCleanupObserved => "terminal_cleanup_observed",
    ThreadBound => "thread_bound",
    ToolCompleted => "tool_completed",
    TurnFinished => "turn_finished",
    TurnStarted => "turn_started",
    EvidenceSubmitted => "evidence_submitted",
    CollectorOverflow => "collector_overflow",
});

domain_enum!(ProviderEventKind {
    ApiKeyRotated => "api_key_rotated",
    LoginLimited => "login_limited",
    LoginProbeInvalid => "login_probe_invalid",
    LoginRestored => "login_restored",
    Started => "started",
    Initialized => "initialized",
    UnsupportedRequest => "unsupported_request",
    Shutdown => "shutdown",
    AccountUpdated => "account_updated",
    RateLimitsUpdated => "rate_limits_updated",
    ConfigurationWarning => "configuration_warning",
    ProviderError => "provider_error",
    ThreadMetadata => "thread_metadata",
    ConnectionNotification => "connection_notification",
});

// Known provider statuses. External unknown values remain in their raw evidence view.
domain_enum!(ProviderTurnStatus {
    Completed => "completed",
    Interrupted => "interrupted",
    Failed => "failed",
    InProgress => "inProgress",
});

impl ProviderTurnStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Interrupted | Self::Failed)
    }
}

impl MessageState {
    /// Classify a turn/completed notification without losing its raw status.
    pub fn from_provider_completion(status: Option<ProviderTurnStatus>) -> Self {
        match status {
            Some(ProviderTurnStatus::Completed) => Self::Completed,
            Some(ProviderTurnStatus::Interrupted) => Self::Interrupted,
            _ => Self::Failed,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Test fixtures and assertions deliberately panic on failure.
mod tests {
    use super::*;
    use rusqlite::{
        Connection, ToSql,
        types::{FromSql, ValueRef},
    };
    use serde::de::DeserializeOwned;
    use std::fmt::Debug;

    fn assert_mapping<T>(values: &[(T, &str)], as_str: impl Fn(T) -> &'static str)
    where
        T: Copy + Debug + PartialEq + Serialize + DeserializeOwned + ToSql + FromSql,
    {
        let connection = Connection::open_in_memory().unwrap();
        for &(value, expected) in values {
            assert_eq!(as_str(value), expected);
            assert_eq!(serde_json::to_value(value).unwrap(), expected);
            assert_eq!(
                serde_json::from_value::<T>(serde_json::json!(expected)).unwrap(),
                value
            );
            assert_eq!(
                connection
                    .query_row("SELECT ?1", [&value], |row| row.get::<_, String>(0))
                    .unwrap(),
                expected
            );
            assert_eq!(
                connection
                    .query_row("SELECT ?1", [expected], |row| row.get::<_, T>(0))
                    .unwrap(),
                value
            );
        }
        for invalid in ["unknown", "", "COMPLETED"] {
            if values.iter().any(|(_, valid)| *valid == invalid) {
                continue;
            }
            assert!(serde_json::from_value::<T>(serde_json::json!(invalid)).is_err());
            assert!(
                connection
                    .query_row("SELECT ?1", [invalid], |row| row.get::<_, T>(0))
                    .is_err()
            );
        }
        for invalid in [
            ValueRef::Null,
            ValueRef::Integer(1),
            ValueRef::Blob(b"stop"),
        ] {
            assert!(T::column_result(invalid).is_err());
        }
    }

    #[test]
    fn additional_persisted_families_keep_current_wire_and_sql_literals() {
        use crate::runtime::auth_fallback::{
            AuthReason, AvailabilityState, ProbeError, WaitingReason,
        };
        assert_mapping(
            &[
                (MessageKind::ApprovalAnswer, "approval_answer"),
                (MessageKind::Continuation, "continuation"),
            ],
            MessageKind::as_str,
        );
        assert_mapping(
            &[
                (WaitingReason::Quota, "quota"),
                (WaitingReason::Switch, "switch"),
            ],
            WaitingReason::as_str,
        );
        assert_mapping(
            &[
                (AvailabilityState::Available, "available"),
                (AvailabilityState::Limited, "limited"),
                (AvailabilityState::Unavailable, "unavailable"),
            ],
            AvailabilityState::as_str,
        );
        assert_mapping(
            &[
                (AuthReason::UsageLimit, "usage_limit"),
                (AuthReason::Authentication, "authentication"),
                (
                    AuthReason::UsageAndAuthentication,
                    "usage_and_authentication",
                ),
                (AuthReason::SpendCap, "spend_cap"),
                (AuthReason::Manual, "manual"),
            ],
            AuthReason::as_str,
        );
        assert_mapping(
            &[
                (ProbeError::MissingInput, "missing_input"),
                (ProbeError::MissingOwner, "missing_owner"),
                (ProbeError::OwnerBindingMismatch, "owner_binding_mismatch"),
            ],
            ProbeError::as_str,
        );
        assert_branch_approval_and_purge_literals();
    }

    fn assert_branch_approval_and_purge_literals() {
        use crate::approvals::{ApprovalAction, ApprovalState};
        use crate::runtime::branching::BranchPhase;
        assert_mapping(
            &[
                (BranchPhase::WaitingForSource, "waiting_for_source"),
                (BranchPhase::ReadyToFork, "ready_to_fork"),
                (BranchPhase::Forking, "forking"),
                (BranchPhase::Bound, "bound"),
                (BranchPhase::Failed, "failed"),
                (BranchPhase::Uncertain, "uncertain"),
            ],
            BranchPhase::as_str,
        );
        assert_mapping(
            &[
                (ApprovalState::Pending, "pending"),
                (ApprovalState::Answered, "answered"),
                (ApprovalState::Superseded, "superseded"),
                (ApprovalState::Withdrawn, "withdrawn"),
            ],
            ApprovalState::as_str,
        );
        assert_mapping(
            &[
                (ApprovalAction::Accept, "accept"),
                (ApprovalAction::Decline, "decline"),
                (ApprovalAction::Cancel, "cancel"),
            ],
            ApprovalAction::as_str,
        );
        assert_mapping(
            &[
                (
                    PurgeBlockerReason::PendingNativeBranches,
                    "pending_native_branches",
                ),
                (PurgeBlockerReason::AgentNotEnded, "agent_not_ended"),
                (
                    PurgeBlockerReason::AgentCleanupPending,
                    "agent_cleanup_pending",
                ),
                (
                    PurgeBlockerReason::AgentCleanupUnconfirmed,
                    "agent_cleanup_unconfirmed",
                ),
                (PurgeBlockerReason::ActiveInput, "active_input"),
                (
                    PurgeBlockerReason::UnresolvedReservation,
                    "unresolved_reservation",
                ),
            ],
            PurgeBlockerReason::as_str,
        );
    }

    macro_rules! audit {
        ($kind:ty, [$($wire:literal),+ $(,)?]) => {
            let expected = [$($wire),+];
            assert_eq!(<$kind>::VALUES, expected);
            let values: Vec<_> = expected.into_iter()
                .map(|wire| (<$kind>::parse(wire).unwrap(), wire)).collect();
            assert_mapping(&values, <$kind>::as_str);
        };
    }

    fn assert_auth_report_literals() {
        audit!(
            crate::auth::AuthOutcome,
            [
                "already_authenticated",
                "already_verified",
                "authenticated",
                "cancelled",
                "generation_replaced",
                "login_failed",
                "login_refused",
                "login_selection_unknown",
                "login_superseded",
                "no_recovery_needed",
                "reconciled",
                "registered",
                "recovery_failed",
                "repair_finalization_failed",
                "repair_refused",
                "repair_required",
                "repair_retired",
                "repair_superseded",
                "timeout",
                "verification_failed",
                "verification_pending",
            ]
        );
        audit!(
            crate::auth::AuthOperation,
            ["status", "list", "login", "recover", "repair"]
        );
        audit!(crate::auth::ObservedMethod, ["subscription", "other"]);
    }

    fn assert_runtime_diagnostic_literals() {
        audit!(
            crate::runtime::provider::ShutdownMethod,
            [
                "not_started",
                "session_released",
                "uncertain",
                "eof",
                "sigterm",
                "forced",
            ]
        );
        audit!(
            crate::runtime::steering_canary::SteeringObservation,
            [
                "proof_invalid",
                "proof_missing",
                "proof_missing_or_revoked",
                "proof_stale_adapter_changed",
                "proof_stale_binary_changed",
                "proof_not_verified_for_model_effort",
                "verified",
            ]
        );
    }

    #[test]
    fn report_and_diagnostic_families_keep_current_wire_literals() {
        assert_auth_report_literals();
        assert_runtime_diagnostic_literals();
    }

    #[test]
    fn recovery_fences_preserve_markers_and_exact_diagnostic_text() {
        for (fence, expected) in [
            (
                RecoveryFence::ExecutionCleanupUncertain,
                "execution_cleanup_uncertain",
            ),
            (
                RecoveryFence::BranchCreationUncertain,
                "branch_creation_uncertain",
            ),
            (
                RecoveryFence::Failure("provider_io: original diagnostic".into()),
                "provider_io: original diagnostic",
            ),
        ] {
            assert_eq!(serde_json::to_value(&fence).unwrap(), expected);
            assert_eq!(
                serde_json::from_value::<RecoveryFence>(serde_json::json!(expected)).unwrap(),
                fence
            );
        }
        for malformed in [
            serde_json::json!(null),
            serde_json::json!(7),
            serde_json::json!({"execution_cleanup_uncertain":null}),
        ] {
            assert!(serde_json::from_value::<RecoveryFence>(malformed).is_err());
        }
    }

    #[test]
    fn agent_state_strings_are_stable() {
        assert_mapping(
            &[
                (AgentState::Starting, "starting"),
                (AgentState::Idle, "idle"),
                (AgentState::Running, "running"),
                (AgentState::Stopping, "stopping"),
                (AgentState::Stopped, "stopped"),
                (AgentState::Failed, "failed"),
            ],
            AgentState::as_str,
        );
    }

    #[test]
    fn task_outcome_strings_are_stable() {
        assert_mapping(
            &[
                (TaskOutcome::Open, "open"),
                (TaskOutcome::Completed, "completed"),
                (TaskOutcome::Cancelled, "cancelled"),
            ],
            TaskOutcome::as_str,
        );
    }

    #[test]
    fn message_state_strings_are_stable() {
        assert_mapping(
            &[
                (MessageState::Accepted, "accepted"),
                (MessageState::Submitting, "submitting"),
                (MessageState::Submitted, "submitted"),
                (MessageState::Completed, "completed"),
                (MessageState::Interrupted, "interrupted"),
                (MessageState::Failed, "failed"),
                (MessageState::Uncertain, "uncertain"),
                (MessageState::Undeliverable, "undeliverable"),
            ],
            MessageState::as_str,
        );
    }

    #[test]
    fn control_strings_are_stable() {
        assert_mapping(
            &[(Control::Stop, "stop"), (Control::Interrupt, "interrupt")],
            Control::as_str,
        );
    }

    #[test]
    fn provider_turn_statuses_keep_protocol_spelling_and_completion_semantics() {
        for (wire, status, message, terminal) in [
            (
                "completed",
                ProviderTurnStatus::Completed,
                MessageState::Completed,
                true,
            ),
            (
                "interrupted",
                ProviderTurnStatus::Interrupted,
                MessageState::Interrupted,
                true,
            ),
            (
                "failed",
                ProviderTurnStatus::Failed,
                MessageState::Failed,
                true,
            ),
            (
                "inProgress",
                ProviderTurnStatus::InProgress,
                MessageState::Failed,
                false,
            ),
        ] {
            let parsed = ProviderTurnStatus::parse(wire);
            assert_eq!(parsed, Some(status));
            assert_eq!(serde_json::to_value(status).unwrap(), wire);
            assert_eq!(
                serde_json::from_value::<ProviderTurnStatus>(serde_json::json!(wire)).unwrap(),
                status
            );
            assert_eq!(MessageState::from_provider_completion(parsed), message);
            assert_eq!(
                parsed.is_some_and(ProviderTurnStatus::is_terminal),
                terminal
            );
        }
        assert_mapping(
            &[
                (ProviderTurnStatus::Completed, "completed"),
                (ProviderTurnStatus::Interrupted, "interrupted"),
                (ProviderTurnStatus::Failed, "failed"),
                (ProviderTurnStatus::InProgress, "inProgress"),
            ],
            ProviderTurnStatus::as_str,
        );
        for unknown in [
            "unknown",
            "",
            "COMPLETED",
            "in_progress",
            "accepted",
            "submitted",
        ] {
            let parsed = ProviderTurnStatus::parse(unknown);
            assert_eq!(parsed, None);
            assert!(
                serde_json::from_value::<ProviderTurnStatus>(serde_json::json!(unknown)).is_err()
            );
            assert_eq!(
                MessageState::from_provider_completion(parsed),
                MessageState::Failed
            );
            assert!(!parsed.is_some_and(ProviderTurnStatus::is_terminal));
        }
    }

    #[test]
    fn event_kind_strings_match_storage() {
        assert_mapping(
            &[
                (EventKind::AssistantMessage, "assistant_message"),
                (EventKind::CompletionRequested, "completion_requested"),
                (EventKind::ControlRequested, "control_requested"),
                (EventKind::ExecutionFailed, "execution_failed"),
                (EventKind::ExecutionStarting, "execution_starting"),
                (EventKind::ExecutionStopped, "execution_stopped"),
                (EventKind::LifetimeExpired, "lifetime_expired"),
                (EventKind::MessageAccepted, "message_accepted"),
                (EventKind::ModelRerouted, "model_rerouted"),
                (EventKind::ProviderError, "provider_error"),
                (
                    EventKind::ProviderSessionReleased,
                    "provider_session_released",
                ),
                (EventKind::ProviderShutdown, "provider_shutdown"),
                (EventKind::ProviderStarted, "provider_started"),
                (EventKind::SessionQuarantined, "session_quarantined"),
                (EventKind::SessionReady, "session_ready"),
                (EventKind::Spawned, "spawned"),
                (EventKind::TaskCompleted, "task_completed"),
                (EventKind::TerminationRequested, "termination_requested"),
                (EventKind::ThreadBound, "thread_bound"),
                (EventKind::ToolCompleted, "tool_completed"),
                (EventKind::TurnFinished, "turn_finished"),
                (EventKind::TurnStarted, "turn_started"),
            ],
            EventKind::as_str,
        );
    }

    #[test]
    fn provider_event_kind_strings_match_storage() {
        assert_mapping(
            &[
                (ProviderEventKind::Started, "started"),
                (ProviderEventKind::UnsupportedRequest, "unsupported_request"),
                (ProviderEventKind::Shutdown, "shutdown"),
                (ProviderEventKind::AccountUpdated, "account_updated"),
                (ProviderEventKind::RateLimitsUpdated, "rate_limits_updated"),
                (
                    ProviderEventKind::ConfigurationWarning,
                    "configuration_warning",
                ),
                (ProviderEventKind::ProviderError, "provider_error"),
                (ProviderEventKind::ThreadMetadata, "thread_metadata"),
                (
                    ProviderEventKind::ConnectionNotification,
                    "connection_notification",
                ),
            ],
            ProviderEventKind::as_str,
        );
    }

    const STORE_SQL_SOURCES: &[&str] = &[
        include_str!("store/schema.sql"),
        include_str!("store/approvals/queries.rs"),
        include_str!("store/artifacts/queries.rs"),
        include_str!("store/auth_fallback/queries.rs"),
        include_str!("store/branching/queries.rs"),
        include_str!("store/calls/queries.rs"),
        include_str!("store/gc/queries.rs"),
        include_str!("store/maintenance/queries.rs"),
        include_str!("store/purge/queries.rs"),
        include_str!("store/purge_scope/queries.rs"),
        include_str!("store/queries.rs"),
        include_str!("store/redirect/queries.rs"),
        include_str!("store/scheduled/queries.rs"),
        include_str!("store/schema/queries.rs"),
        include_str!("store/steering/queries.rs"),
    ];

    const NON_STATE_LITERALS: &[&str] = &[
        // Fixed recovery markers are checked by the RecoveryFence projection test.
        "execution_cleanup_uncertain",
        "branch_creation_uncertain",
        "$.auth_cutoff_turn",
        "$.auth_generation",
        "$.auth_switch_pending.target",
        "$.recovery_fence",
        "body",
        "confirmed_at",
        "generation",
        "id",
        "key",
        "message",
        "not_before",
        "probe_agent",
        "probe_deadline",
        "probe_error",
        "probe_id",
        "probe_message",
        "profile",
        "reason",
        "recipient",
        "retry_at",
        "revision",
        "sender",
        "state",
        "task_outcome",
        "title",
        "until",
        "updated_at",
        "window",
        "turn",
        "",
        "sqlite_*",
        "human",
        "hive",
        "$.role",
        "$.name",
        "$.comb",
        "$.auth",
        "$.auth_group",
        "$.waiting_reason",
        "$.quota_wait_until",
        "$.auth_checked_revision",
        "$.lifetime",
        "$.paused",
        "$.execution_id",
        "$.control",
        "$.completion_pending",
        "$.result",
        "$.error",
        "$.provider_pid",
        "$.provider_spawn_before",
        "$.provider_spawn_after",
        "$.provider_id",
        "$.conversation_clean",
        "$.thread_id",
        "$.turn_id",
        "$.expired",
        "$.ended_at",
        "$.steering_supported",
        "$.turn_selection",
        "$.redirect_pending",
        "$.parent_id",
        "$.branch_hold",
        "$.branch_phase",
        "$.state",
        "$.task_outcome",
        "true",
        "false",
        "with_parent",
        "integer",
        "invalid_delivery_proof",
        "Artifact identity is pending deletion",
        "Join artifact ownership mismatch",
        "Purged agent identities cannot be reused",
        "Agent ending timestamp is immutable",
    ];

    fn unknown_sql_literals(source: &str) -> Vec<&str> {
        // These SQL source files use single quotes only for SQL literals. Audit
        // every spelling, with a narrow inventory for non-state values. Table
        // ownership and SQL semantics remain covered by the storage tests.
        source
            .split('\'')
            .skip(1)
            .step_by(2)
            .filter(|value| {
                AgentState::parse(value).is_none()
                    && TaskOutcome::parse(value).is_none()
                    && MessageState::parse(value).is_none()
                    && DeliveryMode::parse(value).is_none()
                    && RedirectState::parse(value).is_none()
                    && Control::parse(value).is_none()
                    && EventKind::parse(value).is_none()
                    && ProviderTurnStatus::parse(value).is_none()
                    && MessageKind::parse(value).is_none()
                    && crate::runtime::api::joins::JoinState::parse(value).is_none()
                    && crate::runtime::api::joins::JoinOutcome::parse(value).is_none()
                    && crate::runtime::branching::BranchPhase::parse(value).is_none()
                    && crate::approvals::ApprovalState::parse(value).is_none()
                    && crate::approvals::ApprovalAction::parse(value).is_none()
                    && crate::runtime::auth_fallback::AvailabilityState::parse(value).is_none()
                    && crate::runtime::auth_fallback::WaitingReason::parse(value).is_none()
                    && crate::runtime::auth_fallback::AuthReason::parse(value).is_none()
                    && crate::runtime::auth_fallback::ProbeError::parse(value).is_none()
                    && !NON_STATE_LITERALS.contains(value)
            })
            .collect()
    }

    #[test]
    fn state_strings_match_sql_literals() {
        // Other variants are bound as parameters; assert_mapping checks those
        // against the same strings used by JSON and SQLite.
        let schema_values = [
            AgentState::Starting.as_str(),
            AgentState::Idle.as_str(),
            AgentState::Stopped.as_str(),
            TaskOutcome::Completed.as_str(),
            TaskOutcome::Cancelled.as_str(),
            MessageState::Accepted.as_str(),
            Control::Stop.as_str(),
        ];
        let store_values = [
            AgentState::Stopped.as_str(),
            TaskOutcome::Completed.as_str(),
            TaskOutcome::Cancelled.as_str(),
            MessageState::Accepted.as_str(),
            MessageState::Submitting.as_str(),
            MessageState::Submitted.as_str(),
            MessageState::Failed.as_str(),
            MessageState::Uncertain.as_str(),
            MessageState::Undeliverable.as_str(),
            Control::Stop.as_str(),
        ];
        // Retention predicates live in the shared maintenance queries so preview
        // and execution select the same terminal agents.
        let store_source = [
            include_str!("store/queries.rs"),
            include_str!("store/maintenance/queries.rs"),
        ]
        .concat();
        for (source, values) in [
            (include_str!("store/schema.sql"), schema_values.as_slice()),
            (store_source.as_str(), store_values.as_slice()),
        ] {
            for value in values {
                assert!(source.contains(&format!("'{value}'")), "{value}");
            }
        }
        for source in STORE_SQL_SOURCES {
            assert_eq!(unknown_sql_literals(source), Vec::<&str>::new());
        }
    }

    #[test]
    fn sql_literal_inventory_rejects_additional_state_typos() {
        for source in STORE_SQL_SOURCES {
            for (column, typo) in [
                ("state", "stoped"),
                ("task_outcome", "complete"),
                ("control", "interupt"),
                ("kind", "turn_startd"),
            ] {
                let mutated = format!("{source}\nSELECT 1 WHERE {column}='{typo}';");
                assert_eq!(unknown_sql_literals(&mutated), [typo]);
            }
        }
    }
}
