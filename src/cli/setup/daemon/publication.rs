//! Preserve configuration errors across a managed stop, publication and restart.
use super::{Controller, PathBuf, Result, Snapshot};
use crate::error::Error;
use hive_setup_support::service::{Health, RestartFailure};
use std::io;

pub(super) fn paused(
    controller: &Controller<'_>,
    health: &Health<'_>,
    snapshot: &Snapshot,
    text: &str,
) -> Result<Option<PathBuf>> {
    controller
        .paused(health, || snapshot.publish(text).map_err(io::Error::other))
        .map_err(error)
}

fn typed(error: &io::Error) -> Option<&Error> {
    error.get_ref()?.downcast_ref()
}

fn error(cause: io::Error) -> Error {
    if let Some(public) = typed(&cause) {
        return public.clone().with_source(cause);
    }
    let public = if let Some(failure) = cause
        .get_ref()
        .and_then(|e| e.downcast_ref::<RestartFailure>())
    {
        match failure.operation.as_ref().and_then(typed).cloned() {
            Some(mut public) => {
                if let Error::ConfigInvalid { message, .. } | Error::StateIo { message, .. } =
                    &mut public
                {
                    *message = format!(
                        "{message} Daemon restart also failed; inspect service \
                        status before retrying setup."
                    )
                    .into();
                }
                public
            }
            None if failure.operation.is_none() => Error::state_io(
                "Configuration is published, but daemon restart failed; inspect service \
                    status before retrying setup.",
            ),
            None => Error::state_io(
                "Configuration publication and daemon restart failed; inspect service \
                    status before retrying setup.",
            ),
        }
    } else if cause.kind() == io::ErrorKind::InvalidInput {
        Error::config_invalid(
            "Managed configuration publication could not proceed; inspect service \
                status before retrying setup.",
        )
    } else {
        Error::state_io(
            "Managed configuration publication could not proceed; inspect service \
                status before retrying setup.",
        )
    };
    public.with_source(cause)
}

#[cfg(test)]
mod tests;
