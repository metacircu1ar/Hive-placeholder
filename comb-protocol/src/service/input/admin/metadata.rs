use crate::service::input::samples::{self, AdminExample, ClientGrant};
use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Admin {
    Capabilities,
    Health,
    Stats,
    Doctor,
    Sync,
    Import,
    ImportStatus,
    ImportAbandon,
    CredentialCreate,
    CredentialList,
    CredentialRevoke,
}
impl Admin {
    pub fn named(name: &str) -> Option<Self> {
        match name {
            "capabilities" => Some(Self::Capabilities),
            "health" => Some(Self::Health),
            "stats" => Some(Self::Stats),
            "doctor" => Some(Self::Doctor),
            "sync" => Some(Self::Sync),
            "import" => Some(Self::Import),
            "import_status" => Some(Self::ImportStatus),
            "import_abandon" => Some(Self::ImportAbandon),
            "credential_create" => Some(Self::CredentialCreate),
            "credential_list" => Some(Self::CredentialList),
            "credential_revoke" => Some(Self::CredentialRevoke),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Capabilities => "capabilities",
            Self::Health => "health",
            Self::Stats => "stats",
            Self::Doctor => "doctor",
            Self::Sync => "sync",
            Self::Import => "import",
            Self::ImportStatus => "import_status",
            Self::ImportAbandon => "import_abandon",
            Self::CredentialCreate => "credential_create",
            Self::CredentialList => "credential_list",
            Self::CredentialRevoke => "credential_revoke",
        }
    }
    pub fn fields(self) -> &'static [&'static str] {
        match self {
            Self::Import => &["directory", "prefix"],
            Self::ImportAbandon => &["batch_id"],
            Self::CredentialCreate => &["grant"],
            Self::CredentialRevoke => &["id"],
            _ => &[],
        }
    }
    pub fn expected(self, field: &str) -> &'static str {
        match field {
            "directory" => "a directory path string; import reads Markdown files from it",
            "prefix" => super::super::retrieval::Read::Get.expected("slug"),
            "id" => "a credential UUID from credential list",
            "batch_id" => "a canonical import UUID from import --status",
            "grant" => {
                "a client or bridge grant object with its documented fields, each occurring once"
            }
            "grant/kind" => "client or bridge",
            "grant/name" => "a name of 1..512 UTF-8 bytes without control characters",
            "grant/reviewer" => "a boolean; true grants client reviewer access",
            field if field.starts_with("grant/reviewer_roles") => {
                "at most 64 distinct role names, each 1..512 UTF-8 bytes without control \
                    characters"
            }
            _ => "only the documented fields, each occurring once, in this object",
        }
    }
    pub fn sample(self) -> Value {
        match self {
            Self::Import => samples::value(AdminExample::Import {
                op: self.name(),
                directory: "./notes",
                prefix: "concepts",
            }),
            Self::ImportAbandon => samples::value(AdminExample::Batch {
                op: self.name(),
                batch_id: "12345678-1234-4234-8234-123456789abc",
            }),
            Self::CredentialCreate => samples::value(AdminExample::Create {
                op: self.name(),
                grant: ClientGrant {
                    kind: "client",
                    name: "researcher",
                    reviewer: false,
                },
            }),
            Self::CredentialRevoke => samples::value(AdminExample::Revoke {
                op: self.name(),
                id: "12345678-1234-4234-8234-123456789abc",
            }),
            _ => samples::value(AdminExample::Simple { op: self.name() }),
        }
    }
}
