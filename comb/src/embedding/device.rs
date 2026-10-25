//! The serving engine's runtime: always an HTTP endpoint, not vector identity.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeProvider {
    Http,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeInfo {
    pub provider: RuntimeProvider,
    pub max_tokens: Option<usize>,
}
