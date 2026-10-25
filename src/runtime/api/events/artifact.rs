//! Output file references retained in event metadata and compaction manifests.
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct ArtifactReference<'a> {
    pub id: &'a str,
    pub bytes: usize,
    pub format: &'static str,
    pub read_target: String,
    pub read: &'static str,
}
#[derive(Serialize)]
pub(crate) struct ArtifactManifest<'a> {
    pub id: &'a str,
    pub bytes: i64,
    pub created_at: i64,
    pub path: String,
}

#[derive(Serialize)]
pub(crate) struct TextArtifact<'a> {
    pub text: &'a str,
}
