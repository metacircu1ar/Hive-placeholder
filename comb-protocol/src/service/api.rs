//! Shared typed operations. Tool callers cannot choose attribution or authority.
use super::auth::{Claim, Credential, Grant};
use crate::values::{ContentHash, CredentialId, ImportId, RequestId, RunName, Slug};
use crate::{
    Error, Result,
    request::{Consolidate, List},
    search::{Mode, Search},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

pub mod actions;
mod projection;
pub use actions::{Capability, Class, Dispatch, ReadAction};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub protocols: hive_input::compatibility::ProtocolRange,
    pub reviewer: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Capabilities,
    /// Aggregate setup diagnostics; no page contents or agent attribution.
    Health,
    Get {
        slug: Slug,
    },
    Search {
        request: Search,
        #[serde(default)]
        mode: Mode,
    },
    List {
        request: List,
    },
    Links {
        slug: Slug,
        #[serde(default)]
        incoming: bool,
        #[serde(default)]
        after: Option<String>,
        #[serde(default = "link_limit")]
        limit: usize,
    },
    Graph {
        slug: Slug,
        depth: usize,
    },
    Stats,
    Doctor,
    Put {
        slug: Slug,
        markdown: String,
        #[serde(default)]
        expected_hash: Option<ContentHash>,
        request_id: RequestId,
        #[serde(default)]
        run: Option<RunName>,
    },
    Append {
        slug: Slug,
        text: String,
        source: String,
        #[serde(default)]
        summary: Option<String>,
        #[serde(default)]
        expected_hash: Option<ContentHash>,
        request_id: RequestId,
        #[serde(default)]
        run: Option<RunName>,
    },
    Capture {
        text: String,
        request_id: RequestId,
        #[serde(default)]
        run: Option<RunName>,
    },
    Promote {
        slug: Slug,
        expected_hash: ContentHash,
        request_id: RequestId,
        #[serde(default)]
        run: Option<RunName>,
    },
    Consolidate {
        request: Consolidate,
        request_id: RequestId,
        run: RunName,
    },
    Decide {
        slug: Slug,
        expected_hash: ContentHash,
        decision: crate::page::Decision,
        reason: String,
        request_id: RequestId,
        #[serde(default)]
        run: Option<RunName>,
    },
    Remove {
        slug: Slug,
        expected_hash: ContentHash,
        request_id: RequestId,
    },
    Sync,
    Import {
        directory: PathBuf,
        prefix: Option<Slug>,
    },
    ImportStatus,
    ImportAbandon {
        batch_id: ImportId,
    },
    CredentialCreate {
        grant: Grant,
    },
    CredentialList,
    CredentialRevoke {
        id: CredentialId,
    },
}
pub fn link_limit() -> usize {
    super::limits::DEFAULT_LINK_LIMIT
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub protocol: u32,
    pub credential: Credential,
    pub claim: Option<Claim>,
    pub operation: Operation,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum Reply {
    Ok {
        protocol: hive_input::compatibility::Exact<{ crate::compatibility::SERVICE.current }>,
        result: Value,
    },
    Error {
        protocol: hive_input::compatibility::Exact<{ crate::compatibility::SERVICE.current }>,
        error: Error,
    },
}
impl Reply {
    pub fn from_result(result: Result<Value>) -> Self {
        match result {
            Ok(result) => Self::Ok {
                protocol: hive_input::compatibility::Exact,
                result,
            },
            Err(error) => Self::Error {
                protocol: hive_input::compatibility::Exact,
                error,
            },
        }
    }
    pub fn result(self) -> Result<Value> {
        match self {
            Self::Ok { result, .. } => Ok(result),
            Self::Error { error, .. } => Err(error),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Health {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_service: Option<String>,
    pub product_version: String,
    pub compiled_features: Vec<String>,
    pub model: Option<ModelHealth>,
    pub total: u64,
    pub embedded: u64,
    pub pending: u64,
    pub rejected: u64,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct ModelHealth {
    pub space_id: String,
    pub name: String,
    pub revision: String,
    pub backend: String,
    pub dimensions: usize,
    pub query_ready: bool,
}
