//! Public error classifications, messages and retained causes.
// Public errors serialize safe metadata, never retained causes or received input.
const ERROR_FIELDS: usize = 2;

use serde::{Serialize, Serializer, ser::SerializeStruct};
use std::{error::Error as StdError, fmt, sync::Arc};

/// Share causes when cloning while exposing the original error through `source()`.
/// Raw causes may contain provider, configuration or record text; sanitize them before logging.
/// Deliberately not an `StdError`: thiserror must dereference to the actual cause.
#[derive(Clone)]
pub struct ErrorSource {
    cause: Arc<dyn StdError + Send + Sync>,
    kind: SourceKind,
}

#[derive(Clone, Copy)]
enum SourceKind {
    Other,
    Io {
        kind: std::io::ErrorKind,
        raw_os_error: Option<i32>,
    },
    Unsubmitted,
}

impl ErrorSource {
    fn new(cause: impl StdError + Send + Sync + 'static) -> Self {
        // Classify only the immediate source; walking through wrappers would change
        // the callers' recovery decisions. Keep the original cause and source chain.
        let kind = (&cause as &dyn StdError)
            .downcast_ref::<std::io::Error>()
            .map_or(SourceKind::Other, |cause| SourceKind::Io {
                kind: cause.kind(),
                raw_os_error: cause.raw_os_error(),
            });
        Self {
            cause: Arc::new(cause),
            kind,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Input was rejected before provider submission.")]
struct Unsubmitted(#[source] Error);

impl std::ops::Deref for ErrorSource {
    type Target = dyn StdError + Send + Sync;

    fn deref(&self) -> &Self::Target {
        self.cause.as_ref()
    }
}

macro_rules! errors {
    ($($variant:ident => $code:ident),+ $(,)?) => {
        /// Public formatting deliberately excludes raw causes and provider data.
        #[derive(Clone, thiserror::Error)]
        pub enum Error {
            #[error("{input}")]
            Input {
                input: Box<hive_input::InputError>,
                #[source]
                source: Option<ErrorSource>,
            },
            $(
                #[error("{}: {message}", stringify!($code))]
                $variant {
                    message: std::borrow::Cow<'static, str>,
                    #[source]
                    source: Option<ErrorSource>,
                },
            )+
        }

        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum ErrorCode {
            $($variant,)+
            ObservationDeadline,
            Input(hive_input::Code),
        }
        impl ErrorCode {
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => stringify!($code),)+
                    Self::ObservationDeadline => "observation_deadline",
                    Self::Input(code) => code.as_str(),
                }
            }
        }
        impl ErrorCode {
            pub const fn input(code: hive_input::Code) -> Self {
                match code {
                    hive_input::Code::InvalidPath => Self::InvalidPath,
                    code => Self::Input(code),
                }
            }
        }
        impl Serialize for ErrorCode {
            fn serialize<S: Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }
        impl<'de> serde::Deserialize<'de> for ErrorCode {
            fn deserialize<D: serde::Deserializer<'de>>(
                deserializer: D,
            ) -> std::result::Result<Self, D::Error> {
                struct Code;
                impl serde::de::Visitor<'_> for Code {
                    type Value = ErrorCode;
                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str("a Hive error code string")
                    }
                    fn visit_str<E: serde::de::Error>(
                        self,
                        value: &str,
                    ) -> std::result::Result<ErrorCode, E> {
                        match value {
                            $(stringify!($code) => Ok(ErrorCode::$variant),)+
                            "observation_deadline" => Ok(ErrorCode::ObservationDeadline),
                            _ => <hive_input::Code as serde::Deserialize>::deserialize(
                                serde::de::value::StrDeserializer::<E>::new(value)
                            ).map(ErrorCode::input),
                        }
                    }
                }
                deserializer.deserialize_str(Code)
            }
        }

        impl Error {
            $(
                pub const fn $code(message: &'static str) -> Self {
                    Self::$variant { message: std::borrow::Cow::Borrowed(message), source: None }
                }
            )+

            pub const fn kind(&self) -> ErrorCode {
                match self {
                    $(Self::$variant { .. } => ErrorCode::$variant,)+
                    Self::Input { input, .. } => ErrorCode::input(input.code()),
                }
            }
            pub const fn code(&self) -> &'static str {
                self.kind().as_str()
            }
            pub fn is_retryable_restore(&self) -> bool {
                matches!(self, Self::AgentBusy { .. } | Self::DaemonBusy { .. }
                    | Self::ProviderTimeout { .. } | Self::ProviderDraining { .. })
            }

            pub fn message(&self) -> &str {
                match self {
                    $(Self::$variant { message, .. } => message,)+
                    Self::Input { input, .. } => input.message(),
                }
            }

            /// Retain the raw cause internally. Sanitize provider, configuration or record text
            /// before logging a cause obtained through `source()`.
            #[must_use]
            pub fn with_source(mut self, cause: impl StdError + Send + Sync + 'static) -> Self {
                match &mut self {
                    $(Self::$variant { source, .. } => {
                        *source = Some(ErrorSource::new(cause))
                    },)+
                    Self::Input { source, .. } => *source = Some(ErrorSource::new(cause)),
                }
                self
            }

            fn retained_source(&self) -> Option<&ErrorSource> {
                match self {
                    $(Self::$variant { source, .. } => source.as_ref(),)+
                    Self::Input { source, .. } => source.as_ref(),
                }
            }

            fn retained_source_mut(&mut self) -> Option<&mut ErrorSource> {
                match self {
                    $(Self::$variant { source, .. } => source.as_mut(),)+
                    Self::Input { source, .. } => source.as_mut(),
                }
            }
        }

        #[cfg(test)]
        const CONSTRUCTORS: &[(&str, fn(&'static str) -> Error)] = &[
            $((stringify!($code), Error::$code)),+
        ];
    };
}

errors! {
    AttachmentInvalid => attachment_invalid,
    UnknownApproval => unknown_approval,
    ApprovalClosed => approval_closed,
    InvalidResponse => invalid_response,
    UnknownMessage => unknown_message,
    WatchCapacity => watch_capacity,
    ResultExpired => result_expired,
    AgentBusy => agent_busy,
    AgentCompleted => agent_completed,
    AgentPurged => agent_purged,
    PurgeCleanupPending => purge_cleanup_pending,
    AgentLimit => agent_limit,
    ArtifactUnavailable => artifact_unavailable,
    AuthenticationRequired => authentication_required,
    BindingConflict => binding_conflict,
    Cancelled => cancelled,
    CodexBrowserLoginBusy => codex_browser_login_busy,
    CompletionPending => completion_pending,
    ConfigInvalid => config_invalid,
    ConfigUnavailable => config_unavailable,
    DaemonBusy => daemon_busy,
    DaemonProtocol => daemon_protocol_error,
    DaemonStartFailed => daemon_start_failed,
    DaemonStartTimeout => daemon_start_timeout,
    DaemonStopping => daemon_stopping,
    DaemonUnavailable => daemon_unavailable,
    Database => database_error,
    Internal => internal_error,
    LateActivityAfterDrain => late_activity_after_drain,
    EffortMismatch => effort_mismatch,
    HistoryExpired => history_expired,
    InstructionsUnavailable => instructions_unavailable,
    InteractiveTerminalRequired => interactive_terminal_required,
    InterruptTimeout => interrupt_timeout,
    InvalidArtifact => invalid_artifact,
    InvalidArtifactOffset => invalid_artifact_offset,
    InvalidBody => invalid_body,
    InvalidCwd => invalid_cwd,
    InvalidExecutable => invalid_executable,
    InvalidOption => invalid_option,
    InvalidPath => invalid_path,
    InvalidRequest => invalid_request,
    JoinOpen => join_open,
    LoginFailed => login_failed,
    LoginRequired => login_required,
    MalformedProviderOutput => malformed_provider_output,
    MissingBinary => missing_binary,
    ModelMismatch => model_mismatch,
    ModelRerouted => model_rerouted,
    NoActiveTurn => no_active_turn,
    NotApplicable => not_applicable,
    OutputLimit => output_limit,
    PaneBackendUnavailable => pane_backend_unavailable,
    PaneControllerRunning => pane_controller_running,
    ParentRequired => parent_required,
    PipeSetupFailed => pipe_setup_failed,
    PolicyConflict => policy_conflict,
    ProcessWaitFailed => process_wait_failed,
    ProcessVariantLimit => process_variant_limit,
    ProfileBusy => profile_busy,
    Protocol => protocol_error,
    ProviderDraining => provider_draining,
    ProviderExited => provider_exited,
    ProviderFailed => provider_failed,
    ProviderInputRequired => provider_input_required,
    ProviderIo => provider_io,
    ProviderProtocol => provider_protocol_error,
    ProviderRequestFailed => provider_request_failed,
    ProviderShutdownForced => provider_shutdown_forced,
    ProviderTimeout => provider_timeout,
    ProviderUnavailable => provider_unavailable,
    RecoveryRequired => recovery_required,
    RegistrationIncomplete => registration_incomplete,
    RequestConflict => request_conflict,
    DaemonBinaryMismatch => daemon_binary_mismatch,
    IncompatibleProtocol => incompatible_protocol,
    ReplayUnavailable => replay_unavailable,
    SessionMismatch => session_mismatch,
    ShardOutputLimit => shard_output_limit,
    SpawnFailed => spawn_failed,
    StaleExecution => stale_execution,
    StaleTurn => stale_turn,
    SteeringUncertain => steering_uncertain,
    StateInconsistent => state_inconsistent,
    StateIo => state_io,
    Timeout => timeout,
    TurnFailed => turn_failed,
    TurnTimeout => turn_timeout,
    UnknownAgent => unknown_agent,
    UnknownArtifact => unknown_artifact,
    UnknownModel => unknown_model,
    UnknownProfile => unknown_profile,
    UnknownRole => unknown_role,
    UnsafeState => unsafe_state,
    UnsupportedEffort => unsupported_effort,
    UnsupportedOperation => unsupported_operation,
    UnsupportedPlatform => unsupported_platform,
    UnsupportedProvider => unsupported_provider,
    UnsupportedProviderRequest => unsupported_provider_request,
    UnsupportedSchema => unsupported_schema,
    NewerFormat => newer_format,
    UnsupportedTool => unsupported_tool,
    VersionUnrecognized => version_unrecognized,
}

impl From<hive_input::InputError> for Error {
    fn from(input: hive_input::InputError) -> Self {
        Self::Input {
            input: Box::new(input),
            source: None,
        }
    }
}

impl Error {
    pub(crate) fn before_submission(self) -> Self {
        let cause = Unsubmitted(self.clone());
        let mut error = self.with_source(cause);
        if let Some(source) = error.retained_source_mut() {
            source.kind = SourceKind::Unsubmitted;
        }
        error
    }
    pub(crate) fn input_was_not_submitted(&self) -> bool {
        self.retained_source()
            .is_some_and(|source| matches!(source.kind, SourceKind::Unsubmitted))
    }

    /// I/O classification captured at the operation boundary, without walking wrappers.
    pub(crate) fn io_kind(&self) -> Option<std::io::ErrorKind> {
        match self.retained_source()?.kind {
            SourceKind::Io { kind, .. } => Some(kind),
            SourceKind::Other | SourceKind::Unsubmitted => None,
        }
    }

    /// Native errno captured at the operation boundary, preserving exact platform matching.
    pub(crate) fn io_raw_os_error(&self) -> Option<i32> {
        match self.retained_source()?.kind {
            SourceKind::Io { raw_os_error, .. } => raw_os_error,
            SourceKind::Other | SourceKind::Unsubmitted => None,
        }
    }
    pub fn state_io_from(cause: std::io::Error) -> Self {
        Self::state_io("Cannot safely access Hive state.").with_source(cause)
    }

    /// Keep the state writer's I/O classification while retaining publication evidence.
    pub(crate) fn state_publication(cause: atomic_fs::Error) -> Self {
        let kind = SourceKind::Io {
            kind: cause.io_error().kind(),
            raw_os_error: cause.io_error().raw_os_error(),
        };
        let mut error = Self::state_io("Cannot safely access Hive state.").with_source(cause);
        if let Some(source) = error.retained_source_mut() {
            source.kind = kind;
        }
        error
    }

    /// Manifest producers wrap SQL and classified write failures inside I/O.
    /// Preserve that exact public classification along with publication evidence.
    pub(crate) fn manifest_publication(cause: atomic_fs::Error) -> Self {
        let Some(public) = cause
            .io_error()
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<Self>())
            .cloned()
        else {
            return Self::state_publication(cause);
        };
        let kind = public.retained_source().map(|source| source.kind);
        let mut error = public.with_source(cause);
        if let (Some(kind), Some(source)) = (kind, error.retained_source_mut()) {
            source.kind = kind;
        }
        error
    }

    /// Failed sends must release commands, closures and permits immediately.
    #[must_use]
    pub fn with_send_source<T>(self, cause: tokio::sync::mpsc::error::SendError<T>) -> Self {
        drop(cause);
        self.with_source(tokio::sync::mpsc::error::SendError(()))
    }

    /// Preserve Full versus Closed without retaining a charged event payload.
    #[must_use]
    pub fn with_try_send_source<T>(self, cause: tokio::sync::mpsc::error::TrySendError<T>) -> Self {
        use tokio::sync::mpsc::error::TrySendError;
        let source = match cause {
            TrySendError::Full(payload) => {
                drop(payload);
                TrySendError::Full(())
            }
            TrySendError::Closed(payload) => {
                drop(payload);
                TrySendError::Closed(())
            }
        };
        self.with_source(source)
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Error")
            .field("code", &self.code())
            .field("message", &self.message())
            .finish()
    }
}

impl Serialize for Error {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        if let Self::Input { input, .. } = self {
            return input.wire().serialize(serializer);
        }
        let mut value = serializer.serialize_struct("Error", ERROR_FIELDS)?;
        value.serialize_field("code", self.code())?;
        value.serialize_field("message", self.message())?;
        value.end()
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Test fixtures and assertions deliberately panic on failure.
mod tests;
