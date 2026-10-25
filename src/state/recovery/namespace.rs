//! Hold credential namespace ownership and configure its native child receipts.
use super::{
    Lock, Result, Uuid,
    ledger::{Class, Receipt},
};
use std::{os::fd::BorrowedFd, path::PathBuf, process::Command, sync::Arc};

pub(super) struct Namespace {
    pub(crate) lock: Arc<Lock>,
    pub(crate) dir: PathBuf,
    pub(crate) generation: Uuid,
    pub(crate) run: Uuid,
    pub(crate) credential_user: Option<String>,
}
impl Namespace {
    pub(crate) fn fd(&self) -> BorrowedFd<'_> {
        self.lock.fd()
    }
    pub(crate) fn configure(&self, command: &mut Command) {
        if let Some(user) = &self.credential_user {
            command.env("USER", user);
        }
    }
    pub(crate) fn receipt(&self, id: Uuid, class: Class, own_group: bool) -> Result<Receipt> {
        Receipt::create(self, id, class, own_group)
    }
}
