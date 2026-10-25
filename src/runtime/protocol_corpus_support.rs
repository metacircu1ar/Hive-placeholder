//! Provider input corpus and independent expected routing outcomes.
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
pub(super) struct FrameCase {
    pub name: String,
    pub frame: Value,
    pub active: Option<String>,
    pub route: ExpectedRoute,
}

pub(super) fn frames() -> Vec<FrameCase> {
    #[derive(Deserialize)]
    struct Corpus {
        provider: String,
        provider_version: String,
        provenance: String,
        frames: Vec<FrameCase>,
    }
    let mut frames: Vec<FrameCase> =
        serde_json::from_str(include_str!("protocol_frames.json")).unwrap();
    let mut versions = Vec::new();
    for text in [
        include_str!("../provider_support/corpora/codex-0.154.0.json"),
        include_str!("../provider_support/corpora/codex-0.161.0.json"),
    ] {
        let corpus: Corpus = serde_json::from_str(text).unwrap();
        assert_eq!(corpus.provider, "codex");
        assert!(!corpus.provenance.is_empty());
        versions.push(corpus.provider_version.clone());
        frames.extend(corpus.frames.into_iter().map(|mut frame| {
            frame.name = format!("{}:{}", corpus.provider_version, frame.name);
            frame
        }));
    }
    assert_eq!(
        versions,
        crate::provider_support::registry(crate::config::Provider::Codex).corpus_versions
    );
    frames
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ExpectedRoute {
    Response,
    Owner,
    OwnerFailure,
    GlobalFailure,
    Unsupported,
    Notice(NoticeKind),
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum NoticeKind {
    ConnectionNotification,
    ThreadMetadata,
    AccountUpdated,
    RateLimitsUpdated,
    ConfigurationWarning,
    ProviderError,
}
impl NoticeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ConnectionNotification => "connection_notification",
            Self::ThreadMetadata => "thread_metadata",
            Self::AccountUpdated => "account_updated",
            Self::RateLimitsUpdated => "rate_limits_updated",
            Self::ConfigurationWarning => "configuration_warning",
            Self::ProviderError => "provider_error",
        }
    }
}
