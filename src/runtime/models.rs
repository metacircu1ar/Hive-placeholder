//! Owned provider model discovery and cleanup confirmation.
use super::provider;
use crate::{
    config::{Config, Provider},
    error::{Error, Result},
    launch::{self, LauncherCache},
    process,
    state::{self, Binding, Lock, Operation, OperationKind, State},
};
use serde::Serialize;
use serde_json::Value;
use std::path::Path;

// Bound offline discovery even if a provider keeps returning cursors.
const MODEL_PAGE_COUNT: usize = 20;

pub(crate) fn models(config: &Config, state: &State, name: &str, binary: &Path) -> Result<Value> {
    let profile = config.profile(name)?;
    let binding = state
        .lookup(name, profile)?
        .ok_or(Error::login_required("Run auth login first."))?;
    let launcher = launch::system_cache();
    let ModelDiscovery { result, confirmed } =
        models_owned(&launcher, state, &binding, binary, None);
    if !confirmed {
        return Err(recovery_error(result));
    }
    result
}
pub(crate) struct ModelDiscovery {
    pub result: Result<Value>,
    pub confirmed: bool,
}
pub(crate) fn models_owned(
    launcher: &LauncherCache,
    state: &State,
    binding: &Binding,
    binary: &Path,
    owner: Option<&Lock>,
) -> ModelDiscovery {
    let credential = match crate::auth::Credential::resolve(&binding.profile) {
        Ok(credential) => credential,
        Err(error) => {
            return ModelDiscovery {
                result: Err(error),
                confirmed: true,
            };
        }
    };
    models_selected(launcher, state, binding, binary, owner, &credential)
}
pub(crate) fn models_selected(
    launcher: &LauncherCache,
    state: &State,
    binding: &Binding,
    binary: &Path,
    owner: Option<&Lock>,
    credential: &crate::auth::Credential,
) -> ModelDiscovery {
    let operation = state.operation(binding, OperationKind::Models);
    let mut operation = match operation {
        Ok(operation) => operation,
        Err(error) => {
            return ModelDiscovery {
                result: Err(error),
                confirmed: true,
            };
        }
    };
    let result = discover_provider(
        launcher,
        state,
        binding,
        binary,
        owner,
        &mut operation,
        credential,
    );
    if let Err(error) = &result {
        tracing::error!(name: "models.discovery_failed",
             code = error.code(),
             error = error as &(dyn std::error::Error + 'static),
             decision = "finish_operation");
    }
    let confirmed = operation.children_confirmed();
    ModelDiscovery {
        result: finish_discovery(result, operation.finish(), "operation"),
        confirmed,
    }
}

fn discover_provider(
    launcher: &LauncherCache,
    state: &State,
    binding: &Binding,
    binary: &Path,
    owner: Option<&Lock>,
    operation: &mut Operation,
    credential: &crate::auth::Credential,
) -> Result<Value> {
    let pinned = binary.canonicalize().map_err(Error::state_io_from)?;
    let binary = pinned.as_path();
    let compatibility =
        crate::provider_support::observe(launcher, state, binary, binding.profile.provider)?;
    operation.verify_binary(compatibility.identity.clone())?;
    let dir = binding.directory.clone();
    let mut command = process::command(
        binary,
        binding.profile.provider,
        &dir.join("provider"),
        &dir.join("work"),
    );
    credential.configure(binding.profile.provider, &mut command);
    if binding.profile.provider == Provider::Claude {
        let catalog = provider::claude::discover(
            launcher,
            command,
            operation,
            state,
            binding,
            owner.map(Lock::fd),
        )?;
        compatibility.check()?;
        return Ok(catalog);
    }
    let receipt = operation.prepare_child(true, false)?;
    let mut rpc = provider::start_owned_rpc(
        launcher,
        command,
        provider::Backend::Codex,
        &receipt,
        owner.map(Lock::fd),
        || compatibility.check(),
    )?;
    let result = discover_codex(&mut rpc, state, binding, credential);
    let report = rpc.close();
    finish_discovery(result, receipt.finish(report.confirmed), "receipt")
}

fn discover_codex(
    rpc: &mut provider::Rpc,
    state: &State,
    binding: &Binding,
    credential: &crate::auth::Credential,
) -> Result<Value> {
    rpc.initialize_authenticated(credential, &binding.directory.join("provider"))?;
    state.check_account(binding, rpc.account_identity())?;
    model_pages(rpc)
}

fn model_pages(rpc: &mut provider::Rpc) -> Result<Value> {
    let mut cursor = Value::Null;
    let mut data = Vec::new();
    let mut lookup = provider::ModelLookup::default();
    for _ in 0..MODEL_PAGE_COUNT {
        let reply = lookup.page(rpc, &cursor)?;
        cursor = reply.cursor().clone();
        let raw = reply.into_raw();
        data.extend(
            raw["data"]
                .as_array()
                .ok_or_else(|| Error::provider_protocol_error("Invalid model catalog page."))?
                .iter()
                .cloned(),
        );
        if cursor.is_null() {
            return Ok(super::api::value(ModelPage {
                data,
                next_cursor: None,
            }));
        }
    }
    Err(Error::output_limit(
        "Model catalog exceeded the pagination limit.",
    ))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelPage {
    // Every provider entry stays opaque, including unknown native fields.
    data: Vec<Value>,
    next_cursor: Option<()>,
}

// Native failure is primary even when ownership cleanup also fails.
fn finish_discovery<T>(result: Result<T>, cleanup: Result<()>, phase: &'static str) -> Result<T> {
    if let Err(secondary) = &cleanup {
        if let Err(primary) = &result {
            tracing::error!(name: "models.cleanup_failed",
                 phase = phase,
                 code = secondary.code(),
                 primary = primary as &(dyn std::error::Error + 'static),
                 secondary = secondary as &(dyn std::error::Error + 'static),
                 decision = "preserve_discovery_error");
        } else {
            tracing::error!(name: "models.cleanup_failed",
                 phase = phase,
                 code = secondary.code(),
                 error = secondary as &(dyn std::error::Error + 'static),
                 decision = "retain_evidence");
        }
    }
    result.and_then(|value| cleanup.map(|()| value))
}
pub(super) fn recovery_error<T>(result: Result<T>) -> Error {
    let error = state::recovery_required();
    match result {
        Err(cause) => error.with_source(cause),
        Ok(_) => error,
    }
}

#[cfg(test)]
mod failure_tests;
