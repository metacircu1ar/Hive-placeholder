//! Administrative operation diagnostics; authority remains the server's decision.
mod metadata;
mod shape;
#[cfg(test)]
mod tests;
use crate::{Error, Result, service::auth::Grant};
use hive_input::Code;
pub use metadata::Admin;

/// Called only at the registry's existing post-authorization validation boundary.
pub fn validate_grant(grant: &Grant) -> Result<()> {
    grant.validate().map_err(|cause| {
        let (code, field) = invalid_grant_field(grant);
        Admin::CredentialCreate.error(code, &field).caused_by(cause)
    })
}
fn invalid_grant_field(grant: &Grant) -> (Code, String) {
    if !crate::values::bounded(grant.name()) {
        return (Code::InvalidValue, "grant/name".into());
    }
    if let Grant::Bridge { reviewer_roles, .. } = grant {
        if reviewer_roles.len() > crate::service::limits::REVIEWER_ROLES {
            return (Code::OutOfRange, "grant/reviewer_roles".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        for (index, role) in reviewer_roles.iter().enumerate() {
            if !crate::values::bounded(role) || !seen.insert(role) {
                return (Code::InvalidValue, format!("grant/reviewer_roles/{index}"));
            }
        }
    }
    (Code::InvalidValue, "grant".into())
}

impl Admin {
    pub(crate) fn value<T>(self, field: &str, result: Result<T>) -> Result<T> {
        result.map_err(|cause| self.error(Code::InvalidValue, field).caused_by(cause))
    }
    pub fn error(self, code: Code, field: &str) -> Error {
        let mut sample = self.sample();
        if self == Self::CredentialCreate && field.starts_with("grant/reviewer_roles") {
            sample["grant"] =
                serde_json::json!({"kind":"bridge","name":"runtime","reviewer_roles":["reviewer"]});
        }
        match hive_input::InputError::new(
            code,
            "Invalid administrative-operation input.",
            if field.is_empty() {
                "/operation".into()
            } else {
                format!("/operation/{field}")
            },
            self.expected(field),
            vec![format!("operation: {sample}")],
        ) {
            Ok(input) => input.into(),
            Err(cause) => Error::internal_serialization().caused_by(cause),
        }
    }
}
