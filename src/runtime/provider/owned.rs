//! Receipt-owned native RPC launch records authorization and definite spawn absence.
use super::{Backend, Rpc};
use crate::{
    error::{Error, Result},
    launch::LauncherCache,
    state::ChildReceipt,
};
use std::{os::fd::BorrowedFd, process::Command};

pub(crate) fn start_owned_rpc(
    launcher: &LauncherCache,
    command: Command,
    backend: Backend,
    receipt: &ChildReceipt,
    owner: Option<BorrowedFd<'_>>,
    authorize: impl FnOnce() -> Result<()>,
) -> Result<Rpc> {
    let mut inherited = vec![Some(receipt.fd())];
    if let Some(owner) = owner {
        inherited.push(Some(owner));
    }
    match Rpc::start_authorized(launcher, command, backend, inherited, |pid| {
        receipt.started(pid)?;
        authorize()
    }) {
        Ok(rpc) => Ok(rpc),
        Err(error) => {
            if matches!(error, Error::SpawnFailed { .. }) {
                receipt.not_started()?;
            }
            Err(error)
        }
    }
}
