//! Hold credential leases and bind provider execution receipts to them.
use super::{
    Binding, Class, Error, LedgerPhase, Lock, Receipt, Result, State, Uuid, check_writers, corrupt,
    recovery_required,
};
use crate::state::recovery::{FINISH_LOGIN_FIRST, namespace::Namespace};
use std::{os::fd::BorrowedFd, process::Command, sync::Arc};

pub struct RuntimeLease {
    namespace: Namespace,
    account_identity: Option<String>,
}
impl RuntimeLease {
    pub fn verify(&self, state: &State, binding: &Binding) -> Result<()> {
        if binding.id != self.namespace.generation
            || binding.directory.clone() != self.namespace.dir
        {
            return Err(recovery_required());
        }
        if state.verify_credential(binding, &self.namespace.lock)? != self.account_identity
            || state.credential_user(binding)? != self.namespace.credential_user
        {
            return Err(Error::recovery_required(concat!(
                "Credential identity changed while this runtime lease was active; retain ",
                "its evidence and use an isolated generation.",
            )));
        }
        // This same shared credential descriptor has remained held since the
        // original writer check. No exclusive credential writer can appear in
        // that interval; avoid waiting for execution fsyncs on the hub thread.
        Ok(())
    }
    pub fn fd(&self) -> BorrowedFd<'_> {
        self.namespace.fd()
    }
    pub fn verify_account(&self, observed: Option<&str>) -> Result<()> {
        if let (Some(expected), Some(observed)) = (self.account_identity.as_deref(), observed)
            && expected != observed
        {
            return Err(Error::recovery_required(concat!(
                "Native account identity changed for this credential generation. ",
                "Authenticate an isolated replacement; this execution cannot continue.",
            )));
        }
        Ok(())
    }
    pub fn configure(&self, command: &mut Command) {
        self.namespace.configure(command);
    }
    pub fn generation(&self) -> Uuid {
        self.namespace.generation
    }
    pub(crate) fn provider_home(&self) -> std::path::PathBuf {
        self.namespace.dir.join("provider")
    }
    pub fn begin_provider(self: &Arc<Self>, id: &str) -> Result<ProviderEvidence> {
        let id = Uuid::parse_str(id).map_err(|e| corrupt().with_source(e))?;
        Ok(ProviderEvidence {
            receipt: Arc::new(self.namespace.receipt(id, Class::Provider, true)?),
            lease: self.clone(),
        })
    }
}
pub struct ProviderEvidence {
    pub(in crate::state::recovery) receipt: Arc<Receipt>,
    lease: Arc<RuntimeLease>,
}
impl ProviderEvidence {
    pub fn started(&mut self, pid: u32) -> Result<()> {
        self.receipt.started(pid)
    }
    pub fn bind_conversation(&self, agent: &str, execution: &str, native: &str) -> Result<()> {
        self.receipt.update(LedgerPhase::Metadata, |r| {
            r.conversation = Some((agent.into(), execution.into(), native.into()));
            Ok(())
        })
    }
    pub fn account(&self, observed: Option<String>) -> Result<()> {
        self.lease.verify_account(observed.as_deref())?;
        if observed.is_none()
            || self
                .receipt
                .record
                .lock()
                .map_err(|_| corrupt())?
                .account_identity
                == observed
        {
            return Ok(());
        }
        self.receipt.update(LedgerPhase::Metadata, |r| {
            r.account_identity = observed;
            Ok(())
        })
    }
    pub fn finish(&mut self, quiescent: bool, confirmed: bool) -> Result<()> {
        self.receipt.finish(quiescent, confirmed)
    }
}

pub(in crate::state::recovery) fn lease(state: &State, binding: &Binding) -> Result<RuntimeLease> {
    let admission = state.format_admission()?;
    if !binding.ready {
        return Err(Error::registration_incomplete(FINISH_LOGIN_FIRST));
    }
    let dir = binding.directory.clone();
    super::layout::profile(&dir)?;
    // Credential lock precedes every metadata transaction. No upgrades.
    let lock = Arc::new(Lock::shared(&dir.join("auth.lock"))?.with_admission(admission));
    let account_identity = state.verify_credential(binding, &lock)?;
    check_writers(&dir, binding.id)?;
    Ok(RuntimeLease {
        namespace: Namespace {
            lock,
            dir,
            generation: binding.id,
            run: Uuid::new_v4(),
            credential_user: state.credential_user(binding)?,
        },
        account_identity,
    })
}
