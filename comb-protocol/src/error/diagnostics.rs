//! Private diagnostics for the local owner. Never serialize these into replies.
use super::{
    Error, ErrorCode,
    limits::{CAUSE_DEPTH as MAX_CAUSES, DIAGNOSTIC_BYTES as MAX_BYTES},
};
use std::{
    error::Error as _,
    fmt::{self, Write as _},
};

const TRUNCATED: &str = " [truncated]";

struct Bounded(String);
impl fmt::Write for Bounded {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for ch in text.chars() {
            if ch.is_control() {
                for escaped in ch.escape_default() {
                    self.push(escaped)?;
                }
            } else {
                self.push(ch)?;
            }
        }
        Ok(())
    }
}
impl Bounded {
    fn push(&mut self, ch: char) -> fmt::Result {
        if self.0.len() + ch.len_utf8() > MAX_BYTES - TRUNCATED.len() {
            return Err(fmt::Error);
        }
        self.0.push(ch);
        Ok(())
    }
}

impl Error {
    /// Bounded, control-escaped local diagnostics; absent for errors without a cause.
    pub fn diagnostic(&self) -> Option<String> {
        self.source().map(|_| self.render_diagnostic())
    }

    fn render_diagnostic(&self) -> String {
        let mut cause = self.source();
        let mut out = Bounded(String::new());
        let result = (|| {
            write!(out, "{self}")?;
            for _ in 0..MAX_CAUSES {
                let Some(current) = cause else {
                    return Ok(());
                };
                write!(out, "; caused by: {current}")?;
                cause = current.source();
            }
            if cause.is_some() {
                Err(fmt::Error)
            } else {
                Ok(())
            }
        })();
        if result.is_err() {
            out.0.push_str(TRUNCATED);
        }
        out.0
    }

    /// Emit through the caller's tracing subscriber; never format native private text.
    pub fn log_diagnostic(&self) {
        if self.code.is_request_outcome() {
            tracing::debug!(name: "comb.reply",
                 code = self.code.as_str(),
                 error = self as &(dyn std::error::Error + 'static),
                 decision = "reply");
        } else {
            tracing::error!(name: "comb.error",
                 code = self.code.as_str(),
                 error = self as &(dyn std::error::Error + 'static),
                 decision = "observe");
        }
    }
}

impl ErrorCode {
    // Exhaustive so every new code must choose whether it warrants operator attention.
    #[allow(clippy::too_many_lines)] // One exhaustive classification table.
    fn is_request_outcome(self) -> bool {
        match self {
            Self::Input(_)
            | Self::ChunkLimit
            | Self::Conflict
            | Self::ConsolidationConflict
            | Self::ConsolidationLimit
            | Self::CredentialExists
            | Self::CredentialLimit
            | Self::EmbeddingInputRejected
            | Self::EmbeddingInputTooLarge
            | Self::EmbeddingSpaceChanged
            | Self::EmbeddingsBusy
            | Self::CombBusy
            | Self::Forbidden
            | Self::GraphLimit
            | Self::ImmutableTimeline
            | Self::ImportCollision
            | Self::ImportLimit
            | Self::InvalidArguments
            | Self::InvalidCacheBudget
            | Self::InvalidCapture
            | Self::InvalidConsolidation
            | Self::InvalidDecision
            | Self::InvalidDepth
            | Self::InvalidEvidence
            | Self::InvalidFrame
            | Self::InvalidImport
            | Self::InvalidLimit
            | Self::InvalidOperation
            | Self::InvalidRequest
            | Self::InvalidRequestId
            | Self::InvalidSearch
            | Self::InvalidSocket
            | Self::NotFound
            | Self::PageLimit
            | Self::RequestConflict
            | Self::ResultRemoved
            | Self::SocketPathTooLong
            | Self::UnknownTool => true,
            Self::ConnectionLost
            | Self::InvalidEmbeddingAsset
            | Self::EmbeddingAssetSize
            | Self::EmbeddingCredentials
            | Self::EmbeddingEndpointRejected
            | Self::EmbeddingModel
            | Self::EmbeddingReplaceRequired
            | Self::EmbeddingTransport
            | Self::EmbeddingsUnavailable
            | Self::CombUnavailable
            | Self::IndexError
            | Self::InternalError
            | Self::InvalidEmbeddingState
            | Self::InvalidState
            | Self::IoError
            | Self::RecoveryRequired
            | Self::ServerEmbeddingConfig
            | Self::Timeout
            | Self::Uncertain
            | Self::UnsafeCredential
            | Self::UnsafePath
            | Self::UnsafeSocket
            | Self::UnsupportedIndex
            | Self::InvalidEmbedding
            | Self::InvalidEmbeddingBatch
            | Self::InvalidJson
            | Self::InvalidCredential
            | Self::InvalidIdentity
            | Self::InvalidSlug
            | Self::SizeLimit
            | Self::InvalidEmbeddingConfig
            | Self::InvalidActor
            | Self::InvalidPage
            | Self::InvalidTimeline
            | Self::InvalidFrontmatter
            | Self::WorkerFailed
            | Self::UnsupportedFormat
            | Self::NewerFormat
            | Self::IncompatibleProtocol
            | Self::UnknownError => false,
            #[cfg(any(test, feature = "test-support"))]
            Self::FixtureUnavailable | Self::TestInterrupted => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting_bounds_bytes_depth_and_escapes_controls() {
        let text = "private\n\r\t\x1b";
        let error = Error::from(std::io::Error::other(text));
        let rendered = error.diagnostic().unwrap();
        assert!(rendered.contains(r"private\n\r\t\u{1b}"));
        assert!(!rendered.chars().any(char::is_control));
        let huge = Error::from(std::io::Error::other("é".repeat(MAX_BYTES)));
        let rendered = huge.diagnostic().unwrap();
        assert!(rendered.len() <= MAX_BYTES);
        assert!(rendered.ends_with(TRUNCATED));
        let mut deep = Error::not_found();
        for _ in 0..20 {
            deep = Error::not_found().caused_by(deep);
        }
        let rendered = deep.diagnostic().unwrap();
        assert_eq!(rendered.matches("caused by:").count(), MAX_CAUSES);
        assert!(rendered.ends_with(TRUNCATED));
        assert!(Error::not_found().diagnostic().is_none());
    }

    #[test]
    fn tracing_retains_native_metadata_and_redacts_private_causes() {
        use tracing_subscriber::prelude::*;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("errors.jsonl");
        let (backend, guard) = hive_log::start(&path, hive_log::Config::default()).unwrap();
        tracing::subscriber::with_default(tracing_subscriber::registry().with(backend), || {
            Error::internal_serialization()
                .caused_by(Error::from(std::io::Error::from_raw_os_error(libc::EACCES)))
                .log_diagnostic();
            Error::from(std::io::Error::other("secret native error")).log_diagnostic();
            let cause = serde_json::from_str::<u64>("\n\"secret JSON input\"").unwrap_err();
            Error::internal_serialization()
                .caused_by(Error::from(cause))
                .log_diagnostic();
        });
        assert_eq!(
            guard.finish(std::time::Duration::from_secs(1)).state,
            hive_log::Shutdown::Drained
        );
        let log = std::fs::read_to_string(path).unwrap();
        let events: Vec<serde_json::Value> = log
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events.len(), 3);
        assert!(log.contains("PermissionDenied"));
        assert!(log.contains(&format!("\"errno\":{}", libc::EACCES)));
        assert!(log.contains("json_data"));
        assert!(log.contains("\"line\":2"));
        assert!(log.contains("\"column\":19"));
        assert!(!log.contains("secret native error"));
        assert!(!log.contains("secret JSON input"));
    }

    #[test]
    fn expected_replies_are_debug_only_but_io_failures_remain_visible() {
        use tracing::{level_filters::LevelFilter, subscriber::with_default};
        use tracing_subscriber::prelude::*;
        for level in [LevelFilter::INFO, LevelFilter::DEBUG] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("errors.jsonl");
            let config = hive_log::Config {
                level,
                ..Default::default()
            };
            let (backend, guard) = hive_log::start(&path, config).unwrap();
            with_default(tracing_subscriber::registry().with(backend), || {
                Error::not_found().log_diagnostic();
                Error::new(ErrorCode::Conflict, "Private conflict detail").log_diagnostic();
                Error::from(std::io::Error::from_raw_os_error(libc::EACCES)).log_diagnostic();
            });
            assert_eq!(
                guard.finish(std::time::Duration::from_secs(1)).state,
                hive_log::Shutdown::Drained
            );
            let log = std::fs::read_to_string(path).unwrap();
            let events: Vec<serde_json::Value> = log
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(
                events.len(),
                if level == LevelFilter::DEBUG { 3 } else { 1 }
            );
            let failure = events.last().unwrap();
            assert_eq!(failure["level"], "error");
            assert!(log.contains("io_error"));
            assert!(!log.contains("Private conflict detail"));
            if level == LevelFilter::DEBUG {
                for (event, code) in events.iter().zip(["not_found", "conflict"]) {
                    assert_eq!(event["level"], "debug");
                    assert_eq!(event["code"], code);
                    assert_eq!(event["decision"], "reply");
                }
            } else {
                assert!(!log.contains("not_found"));
                assert!(!log.contains("conflict"));
            }
        }
    }

    #[test]
    fn malformed_runtime_output_and_stored_data_are_operational_errors() {
        use tracing_subscriber::prelude::*;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("errors.jsonl");
        let (backend, guard) = hive_log::start(&path, hive_log::Config::default()).unwrap();
        tracing::subscriber::with_default(tracing_subscriber::registry().with(backend), || {
            Error::malformed_embedding_response().log_diagnostic();
            Error::cached_vector_norm().log_diagnostic();
            Error::invalid_completed_batch().log_diagnostic();
            Error::from(serde_json::from_str::<serde_json::Value>("{broken").unwrap_err())
                .log_diagnostic();
            Error::invalid_credential_identity().log_diagnostic();
            Error::invalid_authority_key().log_diagnostic();
            Error::invalid_slug().log_diagnostic();
            Error::file_size_limit().log_diagnostic();
            Error::invalid_embedding_space().log_diagnostic();
        });
        assert_eq!(
            guard.finish(std::time::Duration::from_secs(1)).state,
            hive_log::Shutdown::Drained
        );
        let log = std::fs::read_to_string(path).unwrap();
        let events: Vec<serde_json::Value> = log
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events.len(), 9);
        for (event, code) in events.iter().zip([
            "invalid_embedding",
            "invalid_embedding",
            "invalid_embedding_batch",
            "invalid_json",
            "invalid_credential",
            "invalid_identity",
            "invalid_slug",
            "size_limit",
            "invalid_embedding_config",
        ]) {
            assert_eq!(event["level"], "error");
            assert_eq!(event["code"], code);
        }
    }
}
