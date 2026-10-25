//! Private installation diagnostics live outside the purgeable image manager.
use crate::Result;
use std::{path::Path, sync::Arc, time::Duration};

const DRAIN: Duration = Duration::from_millis(250);

pub(super) fn standalone<T>(work: impl FnOnce() -> Result<T>) -> Result<T> {
    let (backend, guard) = hive_log::start_stderr(hive_log::Config::default())?;
    if let Err(error) = backend.clone().install_global() {
        guard.finish(DRAIN);
        return Err(Box::new(error));
    }
    backend.install_panic_hook();
    let result = work();
    if let Err(error) = &result {
        tracing::error!(name: "install.failed",
             error = error.as_ref() as &(dyn std::error::Error + 'static),
             decision = "preserve_primary_failure");
    }
    guard.finish(DRAIN);
    result
}

/// The worker retains lock ownership even if a blocked disk outlives bounded drain.
pub(super) fn owned<T>(
    prefix: &Path,
    owner: Arc<hive_setup_support::locks::Guard>,
    work: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let directory = prefix.join("lib/hive-install-logs");
    hive_setup_support::files::directory(&directory)?;
    let (backend, guard) = hive_log::start_owned(
        &directory.join("install.jsonl"),
        hive_log::Config::default(),
        owner,
    )?;
    let result = backend.scope(|| {
        tracing::info!(name: "install.lock_acquired", success = true);
        let result = work();
        if let Err(error) = &result {
            tracing::error!(name: "install.failed",
                 error = error.as_ref() as &(dyn std::error::Error + 'static),
                 decision = "retain_recovery_evidence");
        }
        tracing::info!(name: "install.finished", success = result.is_ok());
        result
    });
    // Publication success remains success; a diagnostic failure cannot invite replay.
    guard.finish(DRAIN);
    result
}

pub(super) fn phase(phase: &str) {
    let phase = match phase {
        "before_replace" => "before_replace",
        "after_replace" => "pointer_replaced",
        "before_sync" => "before_sync",
        "after_sync" => "pointer_synced",
        "link_created" => "link_created",
        "after_links" => "links_synced",
        "rollback" => "rollback_started",
        "rollback_sync" => "rollback_sync",
        _ if phase.starts_with("parent_synced:") => "parent_synced",
        _ => return,
    };
    tracing::info!(name: "install.publication_phase", phase = phase);
}
