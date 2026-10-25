//! Wire names are defined once; unknown codes are rejected during deserialization.
use serde::{Deserialize, Serialize};
macro_rules! codes {
    ($($(#[$attr:meta])* $variant:ident => $wire:literal),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum ErrorCode { $($(#[$attr])* $variant),+, Input(hive_input::Code) }
        impl ErrorCode {
            pub const fn as_str(self) -> &'static str {
                match self { $($(#[$attr])* Self::$variant => $wire),
                    +,
                     Self::Input(code) => code.as_str() }
            }
        }
        impl<'de> Deserialize<'de> for ErrorCode {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let name = String::deserialize(d)?;
                match name.as_str() {
                    $($(#[$attr])* $wire => Ok(Self::$variant),)+
                    value => hive_input::Code::deserialize(
                        serde::de::value::StrDeserializer::<D::Error>::new(value)
                    ).map(Self::Input),
                }
            }
        }
    };
}
impl Serialize for ErrorCode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
impl From<hive_input::Code> for ErrorCode {
    fn from(code: hive_input::Code) -> Self {
        match code {
            hive_input::Code::InvalidJson => Self::InvalidJson,
            hive_input::Code::InvalidRequestId => Self::InvalidRequestId,
            code => Self::Input(code),
        }
    }
}
codes! {
    IncompatibleProtocol => "incompatible_protocol",
    UnknownError => "unknown_error",
    CombBusy => "comb_busy",
    CombUnavailable => "comb_unavailable",
    ChunkLimit => "chunk_limit",
    Conflict => "conflict",
    ConnectionLost => "connection_lost",
    ConsolidationConflict => "consolidation_conflict",
    ConsolidationLimit => "consolidation_limit",
    CredentialExists => "credential_exists",
    CredentialLimit => "credential_limit",
    InvalidEmbeddingAsset => "invalid_embedding_asset",
    EmbeddingAssetSize => "embedding_asset_size",
    EmbeddingCredentials => "embedding_credentials",
    EmbeddingEndpointRejected => "embedding_endpoint_rejected",
    EmbeddingInputRejected => "embedding_input_rejected",
    EmbeddingInputTooLarge => "embedding_input_too_large",
    EmbeddingModel => "embedding_model",
    EmbeddingReplaceRequired => "embedding_replace_required",
    EmbeddingSpaceChanged => "embedding_space_changed",
    EmbeddingTransport => "embedding_transport",
    EmbeddingsBusy => "embeddings_busy",
    EmbeddingsUnavailable => "embeddings_unavailable",
    #[cfg(any(test, feature = "test-support"))]
    FixtureUnavailable => "fixture_unavailable",
    Forbidden => "forbidden",
    GraphLimit => "graph_limit",
    ImmutableTimeline => "immutable_timeline",
    ImportCollision => "import_collision",
    ImportLimit => "import_limit",
    IndexError => "index_error",
    InternalError => "internal_error",
    InvalidActor => "invalid_actor",
    InvalidArguments => "invalid_arguments",
    InvalidCacheBudget => "invalid_cache_budget",
    InvalidCapture => "invalid_capture",
    InvalidConsolidation => "invalid_consolidation",
    InvalidCredential => "invalid_credential",
    InvalidDecision => "invalid_decision",
    InvalidDepth => "invalid_depth",
    InvalidEmbedding => "invalid_embedding",
    InvalidEmbeddingBatch => "invalid_embedding_batch",
    InvalidEmbeddingConfig => "invalid_embedding_config",
    InvalidEmbeddingState => "invalid_embedding_state",
    InvalidEvidence => "invalid_evidence",
    InvalidFrame => "invalid_frame",
    InvalidFrontmatter => "invalid_frontmatter",
    InvalidIdentity => "invalid_identity",
    InvalidImport => "invalid_import",
    InvalidJson => "invalid_json",
    InvalidLimit => "invalid_limit",
    InvalidOperation => "invalid_operation",
    InvalidPage => "invalid_page",
    InvalidRequest => "invalid_request",
    InvalidRequestId => "invalid_request_id",
    InvalidSearch => "invalid_search",
    InvalidSlug => "invalid_slug",
    InvalidSocket => "invalid_socket",
    InvalidState => "invalid_state",
    InvalidTimeline => "invalid_timeline",
    IoError => "io_error",
    NotFound => "not_found",
    PageLimit => "page_limit",
    RecoveryRequired => "recovery_required",
    RequestConflict => "request_conflict",
    ResultRemoved => "result_removed",
    ServerEmbeddingConfig => "server_embedding_config",
    SizeLimit => "size_limit",
    SocketPathTooLong => "socket_path_too_long",
    #[cfg(any(test, feature = "test-support"))]
    TestInterrupted => "test_interrupted",
    Timeout => "timeout",
    Uncertain => "uncertain",
    UnknownTool => "unknown_tool",
    UnsafeCredential => "unsafe_credential",
    UnsafePath => "unsafe_path",
    UnsafeSocket => "unsafe_socket",
    UnsupportedIndex => "unsupported_index",
    UnsupportedFormat => "unsupported_format",
    NewerFormat => "newer_format",
    WorkerFailed => "worker_failed",
}
impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
