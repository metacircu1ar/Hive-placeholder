//! Validated domain values. Textual identities retain their exact wire spelling.
use crate::{Actor, Error, Result};
use serde::{Deserialize, Serialize, Serializer};
use std::{fmt, str::FromStr};

mod enums;
pub mod limits;
mod prefix;
pub use enums::{
    CreateSafety, FindingKind, LinkKind, MatchEvidence, ReceiptOperation, Route, Zone,
};
use limits::{FIELD_BYTES, SLUG_BYTES, SLUG_COMPONENT_BYTES};
pub use prefix::Prefix;

macro_rules! text_value {
    ($name:ident, $validate:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
        #[serde(try_from = "String")]
        pub struct $name(String);
        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl TryFrom<String> for $name {
            type Error = Error;
            fn try_from(value: String) -> Result<Self> {
                $validate(&value)?;
                Ok(Self(value))
            }
        }
        impl FromStr for $name {
            type Err = Error;
            fn from_str(value: &str) -> Result<Self> {
                value.to_owned().try_into()
            }
        }
        impl From<&$name> for $name {
            fn from(value: &$name) -> Self {
                value.clone()
            }
        }
        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }
        impl std::borrow::Borrow<str> for $name {
            fn borrow(&self) -> &str {
                self.as_str()
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
        // Borrow the string instead of cloning it during trust/fingerprint serialization.
        impl Serialize for $name {
            fn serialize<S: Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }
        impl rusqlite::types::ToSql for $name {
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
                value
                    .as_str()?
                    .parse()
                    .map_err(|cause| rusqlite::types::FromSqlError::Other(Box::new(cause)))
            }
        }
    };
}
text_value!(Slug, slug);
text_value!(ContentHash, digest);
text_value!(EvidenceId, evidence_id);
text_value!(Timestamp, timestamp);
text_value!(RequestId, request_id);
text_value!(RunName, run_name);
text_value!(Principal, principal);
text_value!(Fingerprint, digest);
text_value!(ActivationId, activation_id);
text_value!(CredentialId, credential_id);
text_value!(ImportId, import_id);

fn slug(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > SLUG_BYTES
        || value.split('/').any(|part| {
            part.is_empty()
                || part.len() > SLUG_COMPONENT_BYTES
                || !part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_".contains(&b))
        })
    {
        return Err(Error::invalid_slug());
    }
    Ok(())
}
fn digest(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::invalid_content_digest());
    }
    Ok(())
}
fn evidence_id(value: &str) -> Result<()> {
    uuid::Uuid::parse_str(value).map_err(|cause| Error::invalid_evidence_id().caused_by(cause))?;
    Ok(())
}
fn timestamp(value: &str) -> Result<()> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map_err(|cause| Error::invalid_evidence_timestamp().caused_by(cause))?;
    Ok(())
}
fn request_id(value: &str) -> Result<()> {
    hive_input::validate::request_id(value)
        .map_err(|cause| Error::invalid_request_id().caused_by(cause))
}
pub fn bounded(value: &str) -> bool {
    !value.is_empty() && value.len() <= FIELD_BYTES && !value.chars().any(char::is_control)
}
fn run_name(value: &str) -> Result<()> {
    hive_input::validate::run_name(value, FIELD_BYTES)
        .map_err(|cause| Error::invalid_request().caused_by(cause))
}
fn activation_id(value: &str) -> Result<()> {
    uuid::Uuid::parse_str(value).map_err(|cause| Error::invalid_active_space().caused_by(cause))?;
    Ok(())
}
fn import_id(value: &str) -> Result<()> {
    let parsed = uuid::Uuid::parse_str(value)
        .map_err(|cause| Error::invalid_import_id().caused_by(cause))?;
    if parsed.to_string() != value {
        return Err(Error::invalid_import_id());
    }
    Ok(())
}
fn credential_id(value: &str) -> Result<()> {
    if value != "human" {
        uuid::Uuid::parse_str(value)
            .map_err(|cause| Error::invalid_credential_identity().caused_by(cause))?;
    }
    Ok(())
}
fn principal(value: &str) -> Result<()> {
    let fields: Vec<String> =
        serde_json::from_str(value).map_err(|cause| Error::invalid_actor().caused_by(cause))?;
    let valid = match fields.as_slice() {
        [kind] => kind == "human",
        [kind, id] => matches!(kind.as_str(), "client" | "collector") && bounded(id),
        [kind, bridge, id] => kind == "agent" && bounded(bridge) && bounded(id),
        _ => false,
    };
    if !valid {
        return Err(Error::invalid_actor());
    }
    Ok(())
}
impl Principal {
    pub fn for_actor(actor: &Actor) -> Result<Self> {
        let fields: Vec<&str> = match actor {
            Actor::Agent {
                bridge,
                id,
                role,
                execution,
                turn,
            } => {
                let mut fields = vec![bridge.as_str(), id, execution, turn];
                fields.extend(role.as_deref());
                fields
            }
            Actor::Collector { bridge } => vec![bridge],
            Actor::Client { id, name } => vec![id, name],
            Actor::Human | Actor::External => Vec::new(),
        };
        if fields.iter().any(|value| !bounded(value)) {
            return Err(Error::invalid_actor());
        }
        let encoded = match actor {
            Actor::Human => crate::error::json_text(&["human"]),
            Actor::Client { id, .. } => crate::error::json_text(&["client", id]),
            Actor::Agent { bridge, id, .. } => crate::error::json_text(&["agent", bridge, id]),
            Actor::Collector { bridge } => crate::error::json_text(&["collector", bridge]),
            Actor::External => return Err(Error::external_mutation_forbidden()),
        }?;
        Ok(Self(encoded))
    }
}
impl ContentHash {
    pub fn caller_input(value: String) -> Result<Self> {
        Self::try_from(value).map_err(|cause| Error::invalid_caller_hash().caused_by(cause))
    }

    pub fn digest(bytes: impl AsRef<[u8]>) -> Self {
        Self(crate::page::hash(bytes))
    }
}
impl Fingerprint {
    pub fn digest(bytes: impl AsRef<[u8]>) -> Self {
        Self(crate::page::hash(bytes))
    }
}
impl EvidenceId {
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}
impl ActivationId {
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}
impl ImportId {
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}
impl RequestId {
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
    pub fn derived(bytes: impl AsRef<[u8]>) -> Self {
        Self(crate::page::hash(bytes))
    }
}
impl Timestamp {
    pub fn search_filter(value: String) -> Result<Self> {
        Self::try_from(value).map_err(|cause| Error::invalid_search_dates().caused_by(cause))
    }

    pub fn now() -> Self {
        Self(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
    }
}
impl CredentialId {
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
    pub fn human() -> Self {
        Self("human".into())
    }
    pub fn is_human(&self) -> bool {
        self.0 == "human"
    }
    pub fn issued(value: String) -> Result<Self> {
        let id = Self::try_from(value)?;
        if id.is_human() {
            return Err(Error::invalid_credential_identity());
        }
        Ok(id)
    }
}

/// Comb identities have always used UUID's canonical human-readable encoding.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(try_from = "String")]
pub struct CombId {
    uuid: uuid::Uuid,
    text: String,
}
impl CombId {
    pub fn generate() -> Self {
        Self::from(uuid::Uuid::new_v4())
    }
    pub fn as_str(&self) -> &str {
        &self.text
    }
}
impl From<uuid::Uuid> for CombId {
    fn from(uuid: uuid::Uuid) -> Self {
        Self {
            text: uuid.to_string(),
            uuid,
        }
    }
}
impl TryFrom<String> for CombId {
    type Error = Error;
    fn try_from(value: String) -> Result<Self> {
        uuid::Uuid::parse_str(&value)
            .map(Self::from)
            .map_err(|cause| Error::invalid_credential_identity().caused_by(cause))
    }
}
impl FromStr for CombId {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self> {
        value.to_owned().try_into()
    }
}
impl From<CombId> for String {
    fn from(value: CombId) -> Self {
        value.text
    }
}
impl fmt::Display for CombId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
impl Serialize for CombId {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
impl rusqlite::types::ToSql for CombId {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(rusqlite::types::ToSqlOutput::Borrowed(
            rusqlite::types::ValueRef::Text(self.as_str().as_bytes()),
        ))
    }
}
