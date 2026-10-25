//! Preflight must precede recovery: a pending activation can change the index.
use super::{Result, ServeSettings, pending_settings, serving_space, settings};
use crate::{
    embedding::Engine,
    service::{Options, Prepared, Server},
};
use std::path::{Path, PathBuf};

/// Retains a validated option set across CLI environment setup. Later selection
/// changes are operational failures, not newly discovered caller-input errors.
pub struct ServePlan {
    root: PathBuf,
    options: Options,
    initial: Option<ServeSettings>,
}
pub fn plan_server(root: &Path, options: Options) -> Result<ServePlan> {
    Server::validate_options(&options)?;
    let initial = if options.embeddings.is_some() && options.socket.is_some() {
        None
    } else {
        match pending_settings(root)? {
            Some(pending) => Some(pending),
            None => settings(root)?,
        }
    };
    options.validate_model(options.embeddings.is_some() || selected(initial.as_ref()))?;
    Ok(ServePlan {
        root: root.into(),
        options,
        initial,
    })
}
pub fn prepare_server(root: &Path, options: Options) -> Result<Prepared> {
    plan_server(root, options)?.prepare()
}
impl ServePlan {
    pub fn prepare(self) -> Result<Prepared> {
        self.prepare_before_open(|| Ok(()))
    }
    pub(super) fn prepare_before_open(
        self,
        before_open: impl FnOnce() -> Result<()>,
    ) -> Result<Prepared> {
        let Self {
            root,
            mut options,
            initial,
        } = self;
        let explicit = options.embeddings.is_some();
        let override_socket = options.socket.take();
        options.socket = override_socket
            .clone()
            .or_else(|| initial.map(|s| s.socket));
        let lease = Server::preflight(&root, &options)?;
        let mut engine = options.embeddings.take().map(Engine::load).transpose()?;
        let model_options = Options {
            replace_embeddings: options.replace_embeddings,
            retry_failed: options.retry_failed,
            vector_cache_mib: options.vector_cache_mib,
            ..Options::default()
        };
        let check_model = |disk: &crate::disk::Disk| {
            if explicit || model_options.model_option().is_none() {
                return Ok(());
            }
            model_options.recheck_model(selected(
                super::selection::settings_under_guard(disk)?.as_ref(),
            ))
        };
        let check = |disk: &crate::disk::Disk| {
            lease.check_disk(disk)?;
            check_model(disk)
        };
        super::selection::recover_checked(&root, &check)?;
        if !explicit {
            options.recheck_model(selected(settings(&root)?.as_ref()))?;
        }
        let stored = settings(&root)?;
        let snapshot = (!explicit)
            .then(|| crate::error::json_bytes(&stored))
            .transpose()?;
        options.recheck_model(explicit || selected(stored.as_ref()))?;
        let expected = if explicit {
            None
        } else {
            stored.as_ref().map(serving_space).transpose()?.flatten()
        };
        options.socket = override_socket.or_else(|| stored.as_ref().map(|s| s.socket.clone()));
        lease.check_path(&crate::service::socket_path(
            &root,
            options.socket.as_deref(),
        )?)?;
        if !explicit {
            engine = stored
                .and_then(|s| s.embeddings)
                .map(Engine::load)
                .transpose()?;
        }
        before_open()?;
        Server::prepare_with_engine(&root, options, expected.as_ref(), engine, lease, &|disk| {
            check_snapshot(disk, snapshot.as_deref())
        })
    }
}
fn selected(settings: Option<&ServeSettings>) -> bool {
    settings.is_some_and(|s| s.embeddings.is_some())
}

fn check_snapshot(disk: &crate::disk::Disk, expected: Option<&[u8]>) -> Result<()> {
    if let Some(expected) = expected {
        let current = super::selection::settings_under_guard(disk)?;
        if crate::error::json_bytes(&current)? != expected {
            return Err(crate::Error::serve_selection_changed());
        }
    }
    Ok(())
}
