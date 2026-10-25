//! Check every distinct configured bridge, retaining the scopes that depend on it.
use super::api;
use super::{Config, Report, Status};
use std::{collections::BTreeMap, path::PathBuf};

pub(super) fn bindings(config: &Config) -> BTreeMap<PathBuf, Vec<String>> {
    let mut paths: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
    if let Some(binding) = &config.comb {
        paths
            .entry(binding.credential_file.clone())
            .or_default()
            .push("default".into());
    }
    for (name, role) in &config.roles {
        if let Some(binding) = &role.comb {
            paths
                .entry(binding.credential_file.clone())
                .or_default()
                .push(format!("role:{name}"));
        }
    }
    for (name, collector) in &config.comb_collectors {
        paths
            .entry(collector.credential_file.clone())
            .or_default()
            .push(format!("collector:{name}"));
    }
    paths
}

pub(super) fn check(config: &Config, report: &mut Report) {
    let bindings = bindings(config);
    if bindings.is_empty() {
        report.add(
            "comb",
            Status::Info,
            api::value("Not configured; optional."),
            vec![],
        );
        return;
    }
    #[cfg(not(feature = "comb"))]
    report.add(
        "comb",
        Status::Error,
        api::value(Unavailable {
            message: "This config requires the Comb-capable Hive build.",
            bindings: &bindings,
        }),
        vec!["hive-install --comb".into()],
    );
    #[cfg(feature = "comb")]
    {
        let mut identities = BTreeMap::new();
        for (path, scopes) in bindings {
            let name = format!("comb:{}", scopes.join(","));
            match probe(&path, &mut identities) {
                Ok(detail) => {
                    report.add(
                        name,
                        detail.status,
                        api::value(BindingHealth {
                            credential_file: &path,
                            scopes: &scopes,
                            socket: &detail.socket,
                            comb_id: &detail.comb_id,
                            health: &detail.health,
                        }),
                        vec![],
                    );
                }
                Err(error) => report.add(
                    name,
                    Status::Error,
                    api::value(BindingFailure {
                        credential_file: &path,
                        scopes: &scopes,
                        error: &error,
                    }),
                    vec!["comb setup --for-hive".into()],
                ),
            }
        }
    }
}

#[cfg(feature = "comb")]
struct Diagnostic {
    status: Status,
    socket: PathBuf,
    comb_id: String,
    health: comb_protocol::service::api::Health,
}

#[cfg(feature = "comb")]
fn probe(
    path: &std::path::Path,
    identities: &mut BTreeMap<PathBuf, String>,
) -> comb_protocol::Result<Diagnostic> {
    use comb_protocol::service::{
        api::{Health, Operation},
        auth::{Claim, Connection},
        client,
    };
    let connection = Connection::read(path)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let value = rt.block_on(client::call(
        &connection,
        &Operation::Health,
        Some(Claim::Collector),
        HEALTH_TIMEOUT,
    ))?;
    let health: Health = serde_json::from_value(value)?;
    // A successful authenticated reply proves this credential's root identity.
    // Detect a socket changing roots during this doctor invocation as well.
    let socket = connection.socket.canonicalize()?;
    let comb_id = connection.credential.comb_id.to_string();
    if identities
        .insert(socket.clone(), comb_id.clone())
        .is_some_and(|old| old != comb_id)
    {
        return Err(comb_protocol::Error::forbidden());
    }
    let status = if health.model.as_ref().is_some_and(|m| !m.query_ready) {
        Status::Error
    } else if health.rejected > 0 {
        Status::Warning
    } else {
        Status::Ok
    };
    Ok(Diagnostic {
        status,
        socket,
        comb_id,
        health,
    })
}

#[cfg(not(feature = "comb"))]
#[derive(serde::Serialize)]
struct Unavailable<'a> {
    message: &'static str,
    bindings: &'a BTreeMap<PathBuf, Vec<String>>,
}
#[cfg(feature = "comb")]
#[derive(serde::Serialize)]
struct BindingHealth<'a> {
    credential_file: &'a std::path::Path,
    scopes: &'a [String],
    socket: &'a std::path::Path,
    comb_id: &'a str,
    health: &'a comb_protocol::service::api::Health,
}
#[cfg(feature = "comb")]
#[derive(serde::Serialize)]
struct BindingFailure<'a> {
    credential_file: &'a std::path::Path,
    scopes: &'a [String],
    error: &'a comb_protocol::Error,
}

#[cfg(feature = "comb")]
const HEALTH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
