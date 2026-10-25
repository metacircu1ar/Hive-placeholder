//! Idempotent root provisioning; preflight never opens or recovers a database.
mod bridge;
pub mod input;
#[cfg(test)]
mod input_tests;
mod intent;
mod paths;
mod plan;
mod prepared;
#[cfg(test)]
mod tests;
mod validate;

use crate::{
    Comb, Error, Result,
    provision::{self, files},
};
pub use plan::Plan;
pub use prepared::Prepared;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Default, clap::Args)]
#[allow(clippy::struct_excessive_bools)] // Independent CLI switches.
pub struct Options {
    /// Install and start this root's user login service (the default).
    #[arg(long, conflicts_with = "no_services")]
    pub services: bool,
    /// Provision files only; keep manual service control.
    #[arg(long)]
    pub no_services: bool,
    /// Private environment file for the service, including HTTP credentials/proxies.
    #[arg(long, conflicts_with = "no_services")]
    pub service_environment: Option<PathBuf>,
    /// Toolchain PATH for the user service; defaults to this shell's PATH.
    #[arg(long, conflicts_with = "no_services")]
    pub service_path: Option<String>,
    /// Serve this HTTP embedding configuration. A fresh root without it uses
    /// text search only; an existing root keeps its selection.
    #[arg(long)]
    pub embedding_config: Option<PathBuf>,
    /// Create or reuse a private bridge credential for `Hive`.
    #[arg(long)]
    pub for_hive: bool,
    #[arg(long, requires = "for_hive")]
    pub bridge_file: Option<PathBuf>,
    #[arg(long, requires = "for_hive")]
    pub reviewer_role: Vec<String>,
    /// Print the plan without initialization, recovery or other writes.
    #[arg(long)]
    pub dry_run: bool,
    /// Never prompt (Comb setup is fully specified by its flags).
    #[arg(long)]
    pub yes: bool,
    /// Installer coordination: existing stopped scopes remain stopped.
    #[arg(long, hide = true)]
    pub preserve_stopped_services: bool,
}
#[derive(Serialize)]
pub struct Outcome {
    pub changed: bool,
    pub dry_run: bool,
    pub plan: Plan,
    pub backfill: &'static str,
    pub service: &'static str,
}
fn invalid(message: impl Into<std::borrow::Cow<'static, str>>) -> Error {
    Error::setup_configuration(message)
}
pub fn default_root() -> Result<PathBuf> {
    paths::home().map(|home| home.join("Comb"))
}
pub(crate) fn bridge_present(plan: &Plan) -> Result<bool> {
    if plan.bridge_file.is_none() {
        return Ok(true);
    }
    bridge::inspect(plan)
}

pub fn run(root: &Path, socket: Option<&Path>, options: Options) -> Result<Outcome> {
    options.prepare()?.run(root, socket)
}

#[cfg(test)]
pub(crate) fn run_offline(root: &Path, socket: Option<&Path>, options: Options) -> Result<Outcome> {
    apply(root, socket, &options.prepare()?)
}

pub(crate) fn apply(root: &Path, socket: Option<&Path>, input: &Prepared) -> Result<Outcome> {
    let options = input.options();
    let plan = Plan::inspect_prepared(root, socket, input)?;
    if options.dry_run {
        return Ok(Outcome {
            changed: false,
            dry_run: true,
            plan,
            backfill: "not started",
            service: "not installed",
        });
    }
    // Existing roots must be idle before any change. A fresh root is not
    // initialized until its endpoint is validated, so a failed setup retries unchanged.
    if !plan.initialize {
        drop(Comb::open(&plan.root)?);
    }
    let mut changed = plan.initialize || plan.recover_activation;
    if let Some(http) = &plan.http {
        if plan.initialize {
            // Validate before creating the knowledge root or its accepted intent.
            provision::validate_http(http.clone())?;
            intent::publish(&plan)?;
            drop(Comb::init(&plan.root)?);
        }
        prepare_socket(&plan)?;
        changed |=
            provision::select_http_for_setup(&plan.root, http.clone(), &plan.socket)?.changed;
    } else if plan.initialize || plan.text_only {
        if plan.initialize {
            intent::publish(&plan)?;
            drop(Comb::init(&plan.root)?);
        }
        prepare_socket(&plan)?;
        changed |= provision::select_none_for_setup(&plan.root, &plan.socket)?.changed;
    } else {
        changed |= provision::reconcile_for_setup(&plan.root)?.changed;
    }
    let mut comb = Comb::open(&plan.root)?;
    let settings = provision::settings(&plan.root)?.ok_or_else(|| {
        invalid("No server settings; pass --embedding-config or select text search.")
    })?;
    changed |= bridge::apply(&mut comb, &plan)?;
    intent::clear(&plan.root).map_err(Error::committed)?;
    Ok(Outcome {
        changed,
        dry_run: false,
        plan,
        backfill: if settings.embeddings.is_some() {
            "pending; serve indexes in background"
        } else {
            "disabled; text search only"
        },
        service: "not installed",
    })
}

fn prepare_socket(plan: &Plan) -> Result<()> {
    use crate::service::socket::directory::{Action, check};
    let parent = plan
        .socket
        .parent()
        .ok_or_else(|| invalid("Socket needs a parent directory."))?;
    check(parent, Action::Create)?;
    Ok(())
}

/// Noncreating server-output admission, before setup changes managed state.
pub fn check_socket_output(path: &Path) -> Result<()> {
    hive_setup_support::names::check_output_name(path)?;
    let path = paths::absolute(path)?;
    if path.as_os_str().as_encoded_bytes().len() > 103 {
        return Err(Error::socket_path_too_long());
    }
    validate::socket_parent(path.parent().ok_or_else(Error::socket_parent_missing)?)
}
