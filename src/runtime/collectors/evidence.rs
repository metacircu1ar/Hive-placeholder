use crate::{
    config::CollectionSource,
    error::{Error, Result},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub(crate) const SUMMARY_BYTES: usize = 8192;

#[cfg(test)]
mod tests;
mod validation;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EvidenceKind {
    Review,
    Correction,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
#[value(rename_all = "snake_case")]
pub(crate) enum Verdict {
    Findings,
    NoFindings,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Evidence {
    /// Evidence category; hosted agents submit review, while correction is human-only.
    pub kind: EvidenceKind,
    /// Explicitly public, nonblank UTF-8 text, at most 8192 bytes; no NUL or private markers.
    pub summary: String,
    #[serde(default)]
    /// At most 64 nonempty public relative labels, each at most 512 bytes; no filesystem access.
    pub files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    /// Optional hexadecimal commit, 7 to 64 ASCII characters.
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Verdict")]
    /// Required for review, forbidden for human-only correction.
    pub verdict: Option<Verdict>,
}
impl Evidence {
    pub fn validate(&self, human: bool) -> Result<CollectionSource> {
        Self::validate_summary(&self.summary).map_err(Error::from)?;
        Self::validate_options(
            self.kind,
            &self.files,
            self.commit.as_deref(),
            self.verdict,
            human,
        )
        .map_err(Error::from)
    }
}
fn private(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    [
        "<private",
        "[private",
        "<!-- private",
        "-----begin",
        ".comb/",
        "auth.json",
        "credentials.json",
    ]
    .iter()
    .any(|marker| value.contains(marker))
}
