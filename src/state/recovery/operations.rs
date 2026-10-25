//! Auth probes share credential use; login mutations retain independent receipts.
use super::{
    Binding, Error, Lock, Result, State, Uuid,
    ledger::{self, Class, Receipt},
    namespace::Namespace,
    recovery_required,
};
use crate::{
    config::Provider,
    process,
    state::{self, recovery::FINISH_LOGIN_FIRST},
};
use std::{
    os::fd::BorrowedFd,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

pub struct Operation {
    binding: Option<Binding>,
    state: State,
    namespace: Namespace,
    children: Vec<ChildReceipt>,
    metadata_attempted: bool,
    binary_proof: Option<crate::runtime::ProviderBinaryIdentity>,
}
#[derive(Clone)]
pub struct ChildReceipt {
    receipt: Arc<Receipt>,
    writer: bool,
    binary_proof: Option<crate::runtime::ProviderBinaryIdentity>,
    mutation: Option<(PathBuf, Uuid, Uuid, Uuid)>,
}
impl ChildReceipt {
    pub fn fd(&self) -> BorrowedFd<'_> {
        self.receipt.lock.fd()
    }
    pub fn started(&self, pid: u32) -> Result<()> {
        if let Some(identity) = &self.binary_proof {
            check_binary(identity)?;
        }
        if let Some((root, generation, run, writer)) = &self.mutation {
            State::new(root.clone())?.mark_credential_attempt(*generation, *run, *writer)?;
        }
        self.receipt.started(pid)
    }
    pub fn not_started(&self) -> Result<()> {
        self.receipt.finish(true, true)?;
        if let Some((root, generation, run, writer)) = &self.mutation {
            // A refused grant may already have persisted the exact writer nonce.
            State::new(root.clone())?.finish_credential_attempt(
                *generation,
                *run,
                *writer,
                None,
                true,
            )?;
        }
        Ok(())
    }
    pub fn finish(&self, confirmed: bool) -> Result<()> {
        self.receipt.finish(confirmed, confirmed)
    }
}
fn check_binary(identity: &crate::runtime::ProviderBinaryIdentity) -> Result<()> {
    if identity.unchanged() {
        Ok(())
    } else {
        Err(Error::provider_unavailable(
            "Provider executable changed before launch.",
        ))
    }
}
impl Operation {
    pub(crate) fn verify_binary(
        &mut self,
        identity: crate::runtime::ProviderBinaryIdentity,
    ) -> Result<()> {
        check_binary(&identity)?;
        self.binary_proof = Some(identity);
        Ok(())
    }

    pub(crate) fn children_confirmed(&self) -> bool {
        self.children.iter().all(|child| child.receipt.confirmed())
    }
    pub fn configure(&self, command: &mut Command) {
        self.namespace.configure(command);
    }
    pub fn fd(&self) -> BorrowedFd<'_> {
        self.namespace.fd()
    }
    pub fn prepare_child(&mut self, own_group: bool, writer: bool) -> Result<ChildReceipt> {
        if writer && self.binding.is_none() {
            return Err(Error::invalid_request(
                "Metadata probes cannot mutate credentials.",
            ));
        }
        if self.binding.is_none() {
            if self.metadata_attempted {
                return Err(Error::invalid_request(
                    "A metadata namespace permits exactly one native probe.",
                ));
            }
            // Receipt creation can publish before reporting a failed parent sync.
            // Consume the attempt before I/O, even if no handle is returned.
            self.metadata_attempted = true;
        }
        let id = Uuid::new_v4();
        let receipt = ChildReceipt {
            receipt: Arc::new(self.namespace.receipt(
                id,
                if writer {
                    Class::CredentialWriter
                } else {
                    Class::Probe
                },
                own_group,
            )?),
            writer,
            binary_proof: self.binary_proof.clone(),
            mutation: writer.then(|| {
                (
                    self.state.root.clone(),
                    self.namespace.generation,
                    self.namespace.run,
                    id,
                )
            }),
        };
        if self.binding.is_none() {
            super::probes::reserve(&self.namespace.dir, id)?;
        }
        self.children.push(receipt.clone());
        Ok(receipt)
    }
    pub fn finish(self) -> Result<()> {
        if self
            .children
            .iter()
            .any(|c| c.writer && !c.receipt.confirmed())
        {
            // A later credential-only resolution can restore eligibility while
            // the original writer's forced cleanup receipt remains unconfirmed.
            ledger::check_writers(&self.namespace.dir, self.namespace.generation)?;
        }
        if self.binding.is_none() {
            let state = State::new(self.state.root.clone())?;
            let _admission = self.namespace.lock.admission();
            let dir = self.namespace.dir.clone();
            let id = self.namespace.run;
            drop(self);
            super::probes::finish(&state, &dir, id);
        }
        Ok(())
    }
    pub fn confirm_account(&self, identity: Option<String>) -> Result<()> {
        let record = self.prepare_resolution()?;
        self.state.finish_credential_attempt(
            self.namespace.generation,
            record.run,
            record.id,
            identity,
            false,
        )
    }
    pub fn confirm_missing(&self) -> Result<()> {
        let record = self.prepare_resolution()?;
        self.state
            .finish_missing_attempt(self.namespace.generation, record.run, record.id)
    }
    pub fn has_pending(&self) -> Result<bool> {
        if self.binding.is_none() {
            return Err(recovery_required());
        }
        Ok(self
            .state
            .pending_credential_attempt(self.namespace.generation)?
            .is_some())
    }
    fn prepare_resolution(&self) -> Result<ledger::Record> {
        let binding = self.binding.as_ref().ok_or_else(recovery_required)?;
        self.verify_probe_ownership()?;
        let record = super::credential::pending(&self.state, binding, &self.namespace.lock)?
            .ok_or_else(recovery_required)?;
        super::credential::persist(&self.namespace.dir, &record)?;
        Ok(record)
    }
    pub(crate) fn verify_probe_ownership(&self) -> Result<()> {
        // The probe used for verification must itself have released ownership;
        // a parsed account reply alone is not a credential-safety boundary.
        let probes = self.children.iter().filter(|child| !child.writer);
        if !probes.clone().any(|child| child.receipt.was_started()) {
            return Err(recovery_required());
        }
        for child in probes {
            if !child.receipt.ownership_gone()? {
                return Err(recovery_required());
            }
        }
        Ok(())
    }
    pub(crate) fn version_command(&self, binary: &Path, provider: Provider) -> Result<Command> {
        let mut command = self.metadata_command(binary, provider)?;
        command.arg("--version");
        Ok(command)
    }
    pub(crate) fn metadata_command(&self, binary: &Path, provider: Provider) -> Result<Command> {
        if self.binding.is_some() {
            return Err(Error::invalid_request(
                "A standalone metadata probe is required.",
            ));
        }
        let command = process::command(
            binary,
            provider,
            &self.namespace.dir.join("provider"),
            &self.namespace.dir.join("work"),
        );
        Ok(command)
    }
}
pub(super) fn begin(
    state: &State,
    binding: &Binding,
    kind: super::OperationKind,
) -> Result<Operation> {
    let admission = state.admission()?;
    state.check_format()?;
    if !binding.ready {
        return Err(Error::registration_incomplete(FINISH_LOGIN_FIRST));
    }
    let dir = binding.directory.clone();
    let pruning = kind == super::OperationKind::Prune;
    if pruning {
        super::layout::audit_profile(&dir)?;
    } else {
        super::layout::profile(&dir)?;
    }
    let reader = kind.is_reader();
    let lock = Arc::new(
        (if reader {
            Lock::shared(&dir.join("auth.lock"))?
        } else if pruning {
            Lock::acquire_existing(&dir.join("auth.lock"))?
        } else {
            Lock::acquire(&dir.join("auth.lock"))?
        })
        .with_admission(admission),
    );
    let pending = if reader {
        state.verify_credential(binding, &lock)?;
        None
    } else {
        if pruning {
            state.verify_prune_lock(binding, &lock)?;
        } else {
            state.verify_reconciliation_lock(binding, &lock)?;
        }
        super::credential::pending(state, binding, &lock)?
    };
    ledger::check_writers_for_operation(&dir, binding.id, pending.as_ref(), pruning)?;
    if !reader {
        // Unknown execution history does not deny shared use. It does prevent
        // mutating that namespace; isolated repair remains available instead.
        for row in ledger::records(&dir)? {
            let safe = if kind == super::OperationKind::Prune && row.class == Class::Provider {
                ledger::conversation_proven(&row)?
            } else {
                ledger::ownership_gone(&row)?
            };
            if !row.confirmed && !safe {
                return Err(recovery_required());
            }
        }
    }
    let credential_user = state.credential_user(binding)?;
    if !reader {
        super::cleanup::profile(state, binding, &lock)?;
    }
    Ok(Operation {
        binding: Some(binding.clone()),
        state: State::new(state.root.clone())?,
        namespace: Namespace {
            lock,
            dir,
            generation: binding.id,
            run: Uuid::new_v4(),
            credential_user,
        },
        children: Vec::new(),
        metadata_attempted: false,
        binary_proof: None,
    })
}

impl State {
    /// Version inspection must remain available while an old profile is locked.
    /// This namespace grants no credential-mutation or account-verification API.
    pub(crate) fn metadata_probe(&self) -> Result<Operation> {
        let admission = self.admission()?;
        self.check_format()?;
        self.sweep_metadata_probes();
        let _initialization = self.initialize()?;
        let _publication = super::probes::publication_lock(self)?;
        let run = Uuid::new_v4();
        let base = self.root.join("native-probes");
        state::directory(&base)?;
        let dir = base.join(run.to_string());
        super::probes::allocate(&dir)?;
        for name in ["provider", "work"] {
            state::directory(&dir.join(name))?;
        }
        let lock = Arc::new(Lock::acquire(&dir.join("auth.lock"))?.with_admission(admission));
        super::probes::publish(&dir, run, &lock)?;
        Ok(Operation {
            binding: None,
            state: State::new(self.root.clone())?,
            namespace: Namespace {
                lock,
                dir,
                generation: run,
                run,
                credential_user: None,
            },
            children: Vec::new(),
            metadata_attempted: false,
            binary_proof: None,
        })
    }
}
