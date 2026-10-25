//! Hosted Comb setup remains a child CLI operation, including in the core build.
use super::{Result, config, invalid, options::Values, parts, plan::Plan};
use crate::config::Config;
use serde::Deserialize;
use serde_yaml_ng::Value;
use std::{
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
    time::Duration,
};
#[derive(Deserialize, serde::Serialize)]
pub(super) struct Details {
    pub root: PathBuf,
    pub bridge_file: Option<PathBuf>,
}
#[derive(Deserialize)]
struct Preview {
    plan: Details,
}
#[derive(Deserialize, serde::Serialize)]
pub(super) struct Applied {
    pub changed: bool,
    pub dry_run: bool,
    pub plan: serde_json::Value,
    pub backfill: String,
    pub service: String,
}
impl Applied {
    pub fn pending_service(&self) -> bool {
        self.service == "stopped; retained" || self.service.starts_with("not managed;")
    }
}
pub(super) struct Comb {
    pub binary: PathBuf,
    pub args: Vec<String>,
    pub details: Details,
    embedding_config: Option<PathBuf>,
    inputs: Vec<&'static str>,
}
impl Comb {
    pub fn inspect(
        options: &Values,
        existing: Option<&Config>,
        skills: &Path,
        plan: &mut Plan,
        timeout: u64,
    ) -> Result<Option<Self>> {
        if !cfg!(feature = "comb") || options.no_comb == Some(true) {
            return Ok(None);
        }
        regular_config(options.embedding_config.as_deref())?;
        let Some(binary) = parts::comb(options.comb_bin.as_deref())? else {
            if options.positive_comb() {
                return Err(invalid(
                    "Requested Comb setup needs an installed Comb binary; pass --comb-bin.",
                ));
            }
            return Ok(None);
        };
        let identity: parts::Identity = parts::call(
            &binary,
            &["version".into(), "--json".into()],
            Duration::from_secs(timeout),
            &[],
        )?;
        if identity.product != "Comb" {
            return Err(invalid("--comb-bin is not an Comb executable."));
        }
        let args = setup_arguments(options, existing)?;
        let mut preview = args.clone();
        preview.push("--dry-run".into());
        let inputs = supplied(options);
        let details =
            parts::call::<Preview>(&binary, &preview, Duration::from_secs(timeout), &inputs)?.plan;
        let credential = details
            .bridge_file
            .as_ref()
            .ok_or_else(|| invalid("Comb setup did not plan a Hive credential."))?;
        add_bindings(options, skills, plan, credential)?;
        // Feature identity was inspected even when an existing stored selection is retained.
        let _ = identity.compiled_features;
        Ok(Some(Self {
            binary,
            args,
            details,
            embedding_config: options.embedding_config.clone(),
            inputs,
        }))
    }
    pub fn apply(&self) -> Result<Applied> {
        regular_config(self.embedding_config.as_deref())?;
        // Downloads and indexing admission have their own bounded native requests.
        parts::progress(&self.binary, &self.args, &self.inputs)
    }
}
// Only caller-supplied values may become CLI input errors. Stored binding paths
// and derived defaults remain operational errors, including during apply.
fn supplied(options: &Values) -> Vec<&'static str> {
    [
        ("--root", options.comb_root.is_some()),
        ("--bridge-file", options.bridge_file.is_some()),
        ("--embedding-config", options.embedding_config.is_some()),
        (
            "--service-environment",
            options.service_environment.is_some(),
        ),
        ("--service-path", options.service_path.is_some()),
    ]
    .into_iter()
    .filter_map(|(flag, present)| present.then_some(flag))
    .collect()
}

fn push(args: &mut Vec<String>, flag: &str, path: Option<&Path>) {
    if let Some(path) = path {
        args.extend([flag.into(), path.to_string_lossy().into_owned()]);
    }
}

fn add_bindings(options: &Values, skills: &Path, plan: &mut Plan, credential: &Path) -> Result<()> {
    let mut roles = std::collections::BTreeMap::new();
    if options.no_comb_roles != Some(true) {
        for (role, file) in [
            ("curator", "comb-curator/SKILL.md"),
            ("comb-reviewer", "hive/SKILL.md"),
        ] {
            let instructions = skills.join(file);
            if !instructions.is_file() {
                return Err(invalid(format!(
                    "Missing bundled skill: {}",
                    instructions.display()
                )));
            }
            roles.insert(
                role,
                SetupRole {
                    provider: plan.default,
                    instructions,
                    comb: (role == "comb-reviewer").then_some(ReviewerCredential {
                        credential_file: credential,
                        reviewer: true,
                    }),
                },
            );
        }
    }
    let comb_collectors = if options.collectors == Some(true) {
        std::collections::BTreeMap::from([(
            "setup",
            SetupCollector {
                credential_file: credential,
                sources: [
                    "review",
                    "correction",
                    "task_completed",
                    "turn_failed",
                    "approval_answer",
                ],
                enabled: true,
            },
        )])
    } else {
        std::collections::BTreeMap::new()
    };
    let wanted = crate::runtime::api::value(SetupBindings {
        comb: SetupCredential {
            credential_file: credential,
        },
        roles,
        comb_collectors,
    });
    let wanted: Value = serde_yaml_ng::to_value(wanted)
        .map_err(|cause| invalid("Cannot encode Comb setup configuration.").with_source(cause))?;
    plan.additions = config::merge(&plan.additions, &wanted)?;
    Ok(())
}

// Capable builds inspect a credential file; core retains the same fallible interface.
#[cfg_attr(not(feature = "comb"), allow(clippy::unnecessary_wraps))]
fn existing_root(options: &Values, existing: Option<&Config>) -> Result<Option<PathBuf>> {
    if options.comb_root.is_some() {
        return Ok(options.comb_root.clone());
    }
    #[cfg(feature = "comb")]
    if let Some(binding) = existing.and_then(|c| c.comb.as_ref()) {
        let connection = comb_protocol::service::auth::Connection::read(&binding.credential_file)
            .map_err(|cause| {
            invalid("Cannot inspect existing Comb binding.").with_source(cause)
        })?;
        if let Some(parent) = connection.socket.parent()
            && parent.file_name() == Some(std::ffi::OsStr::new(".comb"))
        {
            return Ok(parent.parent().map(Path::to_owned));
        }
        return Err(invalid(
            "Existing Comb binding uses a custom socket; pass --comb-root to identify \
                its knowledge root.",
        ));
    }
    #[cfg(not(feature = "comb"))]
    let _ = existing;
    Ok(None)
}

// Hive previews and applies through two child processes; a pipe has one consumer.
fn regular_config(path: Option<&Path>) -> Result<()> {
    let Some(path) = path else { return Ok(()) };
    let opened = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .and_then(|file| file.metadata());
    match opened {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(config_file_error()),
        Err(cause)
            if matches!(
                cause.kind(),
                std::io::ErrorKind::NotFound
                    | std::io::ErrorKind::PermissionDenied
                    | std::io::ErrorKind::InvalidInput
                    | std::io::ErrorKind::NotADirectory
                    | std::io::ErrorKind::IsADirectory
            ) || matches!(
                cause.raw_os_error(),
                Some(
                    libc::ELOOP
                        | libc::ENAMETOOLONG
                        | libc::ENXIO
                        | libc::ENODEV
                        | libc::EOPNOTSUPP
                )
            ) =>
        {
            Err(config_file_error().with_source(cause))
        }
        Err(cause) => Err(super::io(cause)),
    }
}
fn config_file_error() -> crate::error::Error {
    match hive_input::InputError::new(
        hive_input::Code::InvalidFile,
        "Hive setup needs a reusable embedding configuration file.",
        "--embedding-config",
        "a readable regular file; setup reads it in separate preview and apply processes",
        vec!["hivectl setup --embedding-config /private/embeddings.yaml".into()],
    ) {
        Ok(error) => error.into(),
        Err(cause) => super::io(std::io::Error::other(cause)),
    }
}

#[cfg(test)]
mod input_tests;

#[derive(serde::Serialize)]
struct SetupCredential<'a> {
    credential_file: &'a Path,
}
#[derive(serde::Serialize)]
struct ReviewerCredential<'a> {
    credential_file: &'a Path,
    reviewer: bool,
}
#[derive(serde::Serialize)]
struct SetupRole<'a> {
    provider: crate::config::Provider,
    instructions: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    comb: Option<ReviewerCredential<'a>>,
}
#[derive(serde::Serialize)]
struct SetupCollector<'a> {
    credential_file: &'a Path,
    sources: [&'static str; 5],
    enabled: bool,
}
#[derive(serde::Serialize)]
struct SetupBindings<'a> {
    comb: SetupCredential<'a>,
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    roles: std::collections::BTreeMap<&'static str, SetupRole<'a>>,
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    comb_collectors: std::collections::BTreeMap<&'static str, SetupCollector<'a>>,
}

fn setup_arguments(options: &Values, existing: Option<&Config>) -> Result<Vec<String>> {
    let mut args = Vec::new();
    let root = existing_root(options, existing)?;
    push(&mut args, "--root", root.as_deref());
    args.extend(["setup".into(), "--for-hive".into(), "--yes".into()]);
    push(
        &mut args,
        "--embedding-config",
        options.embedding_config.as_deref(),
    );
    // Reuse a configured binding instead of minting a different credential.
    let bridge = options
        .bridge_file
        .as_deref()
        .or_else(|| existing.and_then(|c| c.comb.as_ref().map(|b| b.credential_file.as_path())));
    push(&mut args, "--bridge-file", bridge);
    if options.no_services == Some(true) {
        args.push("--no-services".into());
    }
    if options.preserve_stopped_services == Some(true) {
        args.push("--preserve-stopped-services".into());
    }
    push(
        &mut args,
        "--service-environment",
        options.service_environment.as_deref(),
    );
    if let Some(path) = &options.service_path {
        args.extend(["--service-path".into(), path.clone()]);
    }
    if options.no_comb_roles != Some(true) {
        args.extend(["--reviewer-role".into(), "comb-reviewer".into()]);
    }
    Ok(args)
}
