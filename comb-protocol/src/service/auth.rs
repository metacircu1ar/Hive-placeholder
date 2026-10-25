//! Wire credentials and claims; authority belongs to the server.
use super::limits::{CREDENTIAL_BYTES, REVIEWER_ROLES};
use crate::{
    Error, Result,
    values::{CombId, CredentialId},
};
use serde::{Deserialize, Serialize};
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credential {
    pub comb_id: CombId,
    pub id: CredentialId,
    pub secret: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    pub format: hive_input::compatibility::Exact<{ crate::compatibility::CONNECTION.current }>,
    pub socket: PathBuf,
    pub credential: Credential,
}
impl Connection {
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = crate::fs::read(path, CREDENTIAL_BYTES)?;
        if std::fs::symlink_metadata(path)?.permissions().mode() & 0o077 != 0 {
            return Err(Error::unsafe_credential());
        }
        let shape: serde_json::Value = serde_json::from_slice(&bytes)?;
        if !shape.is_object() {
            return Err(Error::invalid_credential_identity());
        }
        if shape
            .get("format")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|format| format > u64::from(crate::compatibility::CONNECTION.current))
        {
            return Err(Error::newer_format());
        }
        if shape.get("format").and_then(serde_json::Value::as_u64)
            != Some(u64::from(crate::compatibility::CONNECTION.current))
        {
            return Err(Error::unsupported_format());
        }
        if !shape
            .get("credential")
            .is_some_and(serde_json::Value::is_object)
        {
            return Err(Error::invalid_credential_identity());
        }
        // Decode original bytes to retain duplicate-field rejection.
        let config: Self = serde_json::from_slice(&bytes)?;
        if !config.socket.is_absolute() {
            return Err(Error::socket_not_absolute());
        }
        Ok(config)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Grant {
    Client {
        name: String,
        #[serde(default)]
        reviewer: bool,
    },
    Bridge {
        name: String,
        #[serde(default)]
        reviewer_roles: Vec<String>,
    },
}
impl Grant {
    pub fn name(&self) -> &str {
        match self {
            Self::Client { name, .. } | Self::Bridge { name, .. } => name,
        }
    }
    pub fn validate(&self) -> Result<()> {
        bounded(self.name())?;
        if let Self::Bridge { reviewer_roles, .. } = self {
            validate_reviewer_roles(reviewer_roles)?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Claim {
    Agent {
        id: String,
        role: Option<String>,
        execution: String,
        turn: String,
    },
    Collector,
}
fn bounded(s: &str) -> Result<()> {
    if crate::values::bounded(s) {
        Ok(())
    } else {
        Err(Error::invalid_credential_name())
    }
}

/// Validate the raw list before a caller sorts or normalizes it.
pub fn validate_reviewer_roles(roles: &[String]) -> Result<()> {
    if roles.len() > REVIEWER_ROLES {
        return Err(Error::reviewer_role_limit());
    }
    let mut seen = std::collections::BTreeSet::new();
    for role in roles {
        bounded(role)?;
        if !seen.insert(role) {
            return Err(Error::duplicate_reviewer_role());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn connection_format_is_required_and_checked_without_rewriting() {
        let original = serde_json::json!({
            "format": 1,
            "socket": "/tmp/unused.sock",
            "credential": {
                "comb_id": "abcdefab-cdef-4abc-8abc-abcdefabcdef",
                "id": "human",
                "secret": "private-fixture"
            }
        });
        for (format, expected) in [
            (None, Some(crate::ErrorCode::UnsupportedFormat)),
            (
                Some(serde_json::json!(0)),
                Some(crate::ErrorCode::UnsupportedFormat),
            ),
            (
                Some(serde_json::json!(2)),
                Some(crate::ErrorCode::NewerFormat),
            ),
            (
                Some(serde_json::json!("1")),
                Some(crate::ErrorCode::UnsupportedFormat),
            ),
            (
                Some(serde_json::json!(null)),
                Some(crate::ErrorCode::UnsupportedFormat),
            ),
            (Some(serde_json::json!(1)), None),
        ] {
            let mut value = original.clone();
            value.as_object_mut().unwrap().remove("format");
            if let Some(format) = format {
                value["format"] = format;
            }
            let bytes = serde_json::to_vec(&value).unwrap();
            let mut file = tempfile::NamedTempFile::new().unwrap();
            file.write_all(&bytes).unwrap();
            assert_eq!(
                Connection::read(file.path()).err().map(|error| error.code),
                expected
            );
            assert_eq!(std::fs::read(file.path()).unwrap(), bytes);
        }
    }
}
