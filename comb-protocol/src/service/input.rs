//! Decode wire structure before validating caller values, retaining actionable errors.
pub mod admin;
pub mod mutation;
pub mod retrieval;
pub(super) mod samples;
use super::{
    api::{Operation, Request, link_limit},
    auth::{Claim, Credential, Grant},
};
use crate::{
    Result,
    request::List,
    search::{DEFAULT_SEARCH_LIMIT, Mode, Search},
    values::Timestamp,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum RawOperation {
    Capabilities,
    Health,
    Get {
        slug: String,
    },
    Search {
        request: RawSearch,
        #[serde(default)]
        mode: Mode,
    },
    List {
        request: RawList,
    },
    Links {
        slug: String,
        #[serde(default)]
        incoming: bool,
        #[serde(default)]
        after: Option<String>,
        #[serde(default = "link_limit")]
        limit: usize,
    },
    Graph {
        slug: String,
        depth: usize,
    },
    Stats,
    Doctor,
    Put {
        slug: String,
        markdown: String,
        #[serde(default)]
        expected_hash: Option<String>,
        request_id: String,
        #[serde(default)]
        run: Option<String>,
    },
    Append {
        slug: String,
        text: String,
        source: String,
        #[serde(default)]
        summary: Option<String>,
        #[serde(default)]
        expected_hash: Option<String>,
        request_id: String,
        #[serde(default)]
        run: Option<String>,
    },
    Capture {
        text: String,
        request_id: String,
        #[serde(default)]
        run: Option<String>,
    },
    Promote {
        slug: String,
        expected_hash: String,
        request_id: String,
        #[serde(default)]
        run: Option<String>,
    },
    Consolidate {
        request: RawConsolidate,
        request_id: String,
        run: String,
    },
    Decide {
        slug: String,
        expected_hash: String,
        decision: crate::page::Decision,
        reason: String,
        request_id: String,
        #[serde(default)]
        run: Option<String>,
    },
    Remove {
        slug: String,
        expected_hash: String,
        request_id: String,
    },
    Sync,
    Import {
        directory: PathBuf,
        prefix: Option<String>,
    },
    ImportStatus,
    ImportAbandon {
        batch_id: String,
    },
    CredentialCreate {
        grant: Grant,
    },
    CredentialList,
    CredentialRevoke {
        id: String,
    },
}

#[derive(Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct RawSearch {
    query: String,
    #[schemars(extend("minimum" = crate::search::limits::SEARCH_LIMIT_MIN,
         "maximum" = crate::search::limits::SEARCH_LIMIT_MAX))]
    limit: usize,
    #[schemars(with = "String")]
    kind: Option<String>,
    #[schemars(with = "String")]
    tag: Option<String>,
    #[schemars(with = "String")]
    prefix: Option<String>,
    #[schemars(with = "String")]
    after: Option<String>,
    #[schemars(with = "String")]
    before: Option<String>,
}
impl Default for RawSearch {
    fn default() -> Self {
        Self {
            query: String::new(),
            limit: DEFAULT_SEARCH_LIMIT,
            kind: None,
            tag: None,
            prefix: None,
            after: None,
            before: None,
        }
    }
}
impl TryFrom<RawSearch> for Search {
    type Error = crate::Error;
    fn try_from(raw: RawSearch) -> Result<Self> {
        Ok(Self {
            query: raw.query,
            limit: raw.limit,
            kind: raw.kind,
            tag: raw.tag,
            prefix: raw.prefix,
            after: raw.after.map(Timestamp::search_filter).transpose()?,
            before: raw.before.map(Timestamp::search_filter).transpose()?,
        })
    }
}
#[derive(Default, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct RawList {
    #[schemars(with = "String")]
    kind: Option<String>,
    #[schemars(with = "String")]
    tag: Option<String>,
    #[schemars(with = "String")]
    prefix: Option<String>,
    unverified: bool,
    #[schemars(with = "String")]
    after: Option<String>,
    #[schemars(with = "usize",
         extend("minimum" = crate::limits::LIST_LIMIT_MIN,
         "maximum" = crate::limits::LIST_LIMIT_MAX))]
    limit: Option<usize>,
}
impl From<RawList> for List {
    fn from(raw: RawList) -> Self {
        Self {
            kind: raw.kind,
            tag: raw.tag,
            prefix: raw.prefix,
            unverified: raw.unverified,
            after: raw.after,
            limit: raw.limit,
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawConsolidate {
    note: String,
    target: String,
    note_hash: String,
    target_hash: String,
    summary: String,
    #[serde(default)]
    recopy: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCredential {
    comb_id: String,
    id: String,
    secret: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRequest {
    protocol: u32,
    credential: RawCredential,
    claim: Option<Claim>,
    operation: Box<serde_json::value::RawValue>,
}
pub fn request(bytes: &[u8]) -> Result<Request> {
    let shape: serde_json::Value = serde_json::from_slice(bytes)?;
    if !shape.is_object() {
        return Err(crate::Error::request_not_object());
    }
    // Decode original bytes too: inspecting the shape must not erase duplicates.
    let raw: RawRequest = serde_json::from_slice(bytes)?;
    if !crate::compatibility::SERVICE_RANGE.contains(raw.protocol) {
        return Err(crate::Error::incompatible_protocol());
    }
    let value: serde_json::Value = serde_json::from_str(raw.operation.get())?;
    let read = value
        .get("op")
        .and_then(serde_json::Value::as_str)
        .and_then(retrieval::Read::named)
        .filter(|read| *read != retrieval::Read::Backlinks);
    // Keep native structure checks before identity/domain conversion.
    let native = if read.is_none() {
        Some(decode_native(raw.operation.get(), &value)?)
    } else {
        None
    };
    let credential = Credential {
        comb_id: raw.credential.comb_id.try_into()?,
        id: raw.credential.id.try_into()?,
        secret: raw.credential.secret,
    };
    let operation = if let Some(read) = read {
        let fields = value
            .as_object()
            .ok_or_else(crate::Error::internal_serialization)?;
        let operation = read.socket(fields)?;
        // Retain the strict original-byte decoder for duplicate operation keys.
        serde_json::from_str::<RawOperation>(raw.operation.get())?;
        operation
    } else {
        native
            .ok_or_else(crate::Error::internal_serialization)?
            .try_into()?
    };
    Ok(Request {
        protocol: raw.protocol,
        credential,
        claim: raw.claim,
        operation,
    })
}
fn decode_native(bytes: &str, value: &serde_json::Value) -> Result<RawOperation> {
    let name = value.get("op").and_then(serde_json::Value::as_str);
    let admin = name.and_then(admin::Admin::named);
    let raw = serde_json::from_str::<RawOperation>(bytes).map_err(|cause| {
        if let Some(fields) = value.as_object() {
            if let Some(admin) = admin {
                return admin.shape_error(fields, cause.into());
            }
            if let Some(mutation) = name.and_then(mutation::Write::named) {
                return mutation.shape_error(fields, false, cause.into());
            }
        }
        cause.into()
    })?;
    if let Some((admin, fields)) = admin.zip(value.as_object()) {
        admin.check_fields(fields)?;
    }
    Ok(raw)
}
impl TryFrom<RawOperation> for Operation {
    type Error = crate::Error;
    fn try_from(raw: RawOperation) -> Result<Self> {
        Ok(match raw {
            RawOperation::Capabilities => Self::Capabilities,
            RawOperation::Health => Self::Health,
            RawOperation::Get { slug } => Self::Get {
                slug: slug.try_into()?,
            },
            RawOperation::Search { request, mode } => Self::Search {
                request: request.try_into()?,
                mode,
            },
            RawOperation::List { request } => Self::List {
                request: request.into(),
            },
            RawOperation::Links {
                slug,
                incoming,
                after,
                limit,
            } => Self::Links {
                slug: slug.try_into()?,
                incoming,
                after,
                limit,
            },
            RawOperation::Graph { slug, depth } => Self::Graph {
                slug: slug.try_into()?,
                depth,
            },
            RawOperation::Stats => Self::Stats,
            RawOperation::Doctor => Self::Doctor,
            RawOperation::Sync => Self::Sync,
            RawOperation::Import { directory, prefix } => Self::Import {
                directory,
                prefix: admin::Admin::Import
                    .value("prefix", prefix.map(TryInto::try_into).transpose())?,
            },
            RawOperation::ImportStatus => Self::ImportStatus,
            RawOperation::ImportAbandon { batch_id } => Self::ImportAbandon {
                batch_id: admin::Admin::ImportAbandon.value("batch_id", batch_id.try_into())?,
            },
            RawOperation::CredentialCreate { grant } => Self::CredentialCreate { grant },
            RawOperation::CredentialList => Self::CredentialList,
            RawOperation::CredentialRevoke { id } => Self::CredentialRevoke {
                id: admin::Admin::CredentialRevoke.value("id", id.try_into())?,
            },
            mutation => return self::mutation::operation(mutation, false),
        })
    }
}
