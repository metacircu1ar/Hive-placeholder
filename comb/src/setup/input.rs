//! Pure setup rules shared by native preparation and the CLI.
use super::{Error, Options, Result};
use hive_input::{Code, InputError};

pub const EXAMPLE: &str = "comb setup --no-services";

impl Options {
    /// Validate caller syntax and relationships without filesystem or service access.
    pub fn validate_input(&self) -> Result<()> {
        if let Some(path) = &self.bridge_file {
            hive_setup_support::names::check_output_name(path).map_err(|cause| {
                error(
                    Code::InvalidPath,
                    "bridge_file",
                    "a filename outside the reserved .hive-tmp-state- and .comb-tmp-admin- \
                        preparation namespaces",
                )
                .caused_by(cause)
            })?;
        }
        comb_protocol::service::auth::validate_reviewer_roles(&self.reviewer_role).map_err(
            |cause| {
                error(
                Code::InvalidValue,
                "reviewer_role",
                "at most 64 unique role names, each 1..512 UTF-8 bytes without control characters",
            )
            .caused_by(cause)
            },
        )?;
        self.relations()
    }

    fn relations(&self) -> Result<()> {
        for (present, field) in [
            (self.services, "services"),
            (self.service_environment.is_some(), "service_environment"),
            (self.service_path.is_some(), "service_path"),
        ] {
            if self.no_services && present {
                return Err(error(
                    Code::ConflictingArguments,
                    field,
                    "omit this service option with --no-services",
                ));
            }
        }
        for (present, field) in [
            (self.bridge_file.is_some(), "bridge_file"),
            (!self.reviewer_role.is_empty(), "reviewer_role"),
        ] {
            if present && !self.for_hive {
                return Err(error(
                    Code::MissingArgument,
                    field,
                    "--for-hive with bridge options",
                ));
            }
        }
        Ok(())
    }
}

pub(super) fn error(code: Code, field: &str, expected: &str) -> Error {
    match InputError::new(
        code,
        "Invalid setup input.",
        format!("/options/{field}"),
        expected,
        vec![EXAMPLE.into()],
    ) {
        Ok(input) => input.into(),
        Err(cause) => Error::internal_serialization().caused_by(cause),
    }
}

/// Map only coordinates owned by the setup input validator.
pub fn field(pointer: &str) -> Option<&'static str> {
    Some(match pointer {
        "/options/reviewer_role" => "--reviewer-role",
        "/options/services" => "--services",
        "/options/service_environment" => "--service-environment",
        "/options/service_path" => "--service-path",
        "/options/bridge_file" => "--bridge-file",
        "/options/embedding_config" => "--embedding-config",
        _ => return None,
    })
}
