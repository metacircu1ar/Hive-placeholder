//! Validate and record credential identity and mutation ownership.
const ACCOUNT_IDENTITY_CHANGED: &str =
    "Account identity changed; authenticate an isolated replacement generation.";

use super::{
    Binding, Error, GenerationState, Lock, PendingCredentialAttempt, Result, State, corrupt,
};
use uuid::Uuid;

impl State {
    /// Called while holding the credential lock, then the short index transaction.
    /// A replaced lock inode cannot grant access to a previously used namespace.
    pub fn verify_credential(&self, binding: &Binding, lock: &Lock) -> Result<Option<String>> {
        self.verify_credential_access(binding, lock, false)
    }
    pub(crate) fn verify_reconciliation_lock(
        &self,
        binding: &Binding,
        lock: &Lock,
    ) -> Result<Option<String>> {
        self.verify_credential_access(binding, lock, true)
    }
    fn verify_credential_access(
        &self,
        binding: &Binding,
        lock: &Lock,
        pending: bool,
    ) -> Result<Option<String>> {
        let _metadata = self.metadata_lock()?;
        self.verify_generation(binding)?;
        let identity = super::LockIdentity::from_fd(lock.fd())?;
        if super::LockIdentity::at(&binding.directory.join("auth.lock"))? != identity {
            return Err(super::recovery_required());
        }
        let generation = self.read_index()?.into_generation(binding.id)?;
        if generation.credential_fence {
            return Err(Error::recovery_required(concat!(
                "This generation's native account changed. Authenticate an isolated ",
                "replacement with auth repair --reauthenticate.",
            )));
        }
        if generation.credential_mutation.is_some() && !pending {
            return Err(Error::recovery_required(concat!(
                "Login account verification is pending. Retry auth recover or auth login;",
                " unresolved writer ownership may require isolated auth repair ",
                "--reauthenticate.",
            )));
        }
        if !matches!(
            generation.state,
            GenerationState::Active | GenerationState::Retired | GenerationState::Candidate
        ) || generation.binding.directory != binding.directory
        {
            return Err(corrupt());
        }
        let account = generation.account_identity;
        if generation.credential_lock.as_ref() != Some(&identity) {
            return Err(super::recovery_required());
        }
        Ok(account)
    }

    pub fn record_account(&self, binding: &Binding, identity: Option<String>) -> Result<()> {
        self.update_index(|index| {
            let generation = index.generation_mut(binding.id)?;
            self.verify_generation(binding)?;
            if let (Some(expected), Some(observed)) = (&generation.account_identity, &identity)
                && expected != observed
            {
                generation.credential_fence = true;
                generation.possibly_authenticated = true;
                index.save()?;
                return Err(Error::recovery_required(ACCOUNT_IDENTITY_CHANGED));
            }
            generation.authenticated = true;
            generation.ever_authenticated = true;
            generation.possibly_authenticated = true;
            if identity.is_some() {
                generation.account_identity = identity;
            }
            index.save()
        })
    }

    pub fn credential_user(&self, binding: &Binding) -> Result<Option<String>> {
        let _metadata = self.metadata_lock()?;
        Ok(self
            .read_index()?
            .into_generation(binding.id)?
            .keychain_account)
    }
    pub(crate) fn pending_credential_attempt(
        &self,
        id: Uuid,
    ) -> Result<Option<PendingCredentialAttempt>> {
        let _metadata = self.metadata_lock()?;
        let index = self.read_index()?;
        let g = index.generation(id)?;
        g.credential_mutation
            .map(|run| {
                g.credential_writer
                    .map(|writer| PendingCredentialAttempt { run, writer })
                    .ok_or_else(super::recovery_required)
            })
            .transpose()
    }
    pub(crate) fn mark_credential_attempt(
        &self,
        id: Uuid,
        mutation: Uuid,
        writer: Uuid,
    ) -> Result<()> {
        self.update_index(|index| {
            let generation = index.generation_mut(id)?;
            if generation
                .credential_mutation
                .is_some_and(|old| old != mutation || generation.credential_writer != Some(writer))
            {
                return Err(super::recovery_required());
            }
            generation.possibly_authenticated = true;
            generation.credential_mutation = Some(mutation);
            generation.credential_writer = Some(writer);
            index.save()
        })
    }
    pub(crate) fn finish_credential_attempt(
        &self,
        id: Uuid,
        mutation: Uuid,
        writer: Uuid,
        identity: Option<String>,
        not_started: bool,
    ) -> Result<()> {
        self.update_index(|index| {
            let g = index.generation_mut(id)?;
            if not_started && g.credential_mutation.is_none() {
                return Ok(());
            }
            if g.credential_mutation != Some(mutation) || g.credential_writer != Some(writer) {
                return Err(super::recovery_required());
            }
            if !not_started {
                if g.account_identity.is_some()
                    && identity.is_some()
                    && g.account_identity != identity
                {
                    g.credential_fence = true;
                    index.save()?;
                    return Err(Error::recovery_required(concat!(
                        "The login account changed. Authenticate an isolated replacement with ",
                        "auth repair --reauthenticate.",
                    )));
                }
                if g.account_identity.is_some() && identity.is_none() {
                    return Err(Error::recovery_required(concat!(
                        "The provider omitted the account identity needed to compare with saved ",
                        "credentials. Retry auth recover; use isolated repair if identity remains",
                        " unavailable.",
                    )));
                }
                if (g.authenticated || g.ever_authenticated) && g.account_identity.is_none() {
                    return Err(Error::recovery_required(concat!(
                        "The login account identity could not be verified. Retry auth recover; ",
                        "authenticate an isolated replacement if its identity remains ",
                        "unavailable.",
                    )));
                }
                g.account_identity = identity;
                g.authenticated = true;
                g.ever_authenticated = true;
            }
            g.credential_mutation = None;
            g.credential_writer = None;
            index.save()
        })
    }
    /// Status is a shared reader: it must not establish new identity expectations
    /// while providers can hold leases with the previous expectations.
    pub fn check_account(&self, binding: &Binding, observed: Option<&str>) -> Result<()> {
        let _metadata = self.metadata_lock()?;
        let index = self.read_index()?;
        let generation = index.generation(binding.id)?;
        if let (Some(expected), Some(observed)) = (generation.account_identity.as_deref(), observed)
            && expected != observed
        {
            return Err(Error::recovery_required(ACCOUNT_IDENTITY_CHANGED));
        }
        Ok(())
    }
    pub(crate) fn credential_fenced(&self, binding: &Binding) -> Result<bool> {
        let _metadata = self.metadata_lock()?;
        Ok(self.read_index()?.generation(binding.id)?.credential_fence)
    }

    pub(crate) fn finish_missing_attempt(
        &self,
        id: Uuid,
        mutation: Uuid,
        writer: Uuid,
    ) -> Result<()> {
        self.update_index(|index| {
            let g = index.generation_mut(id)?;
            if g.credential_mutation != Some(mutation)
                || g.credential_writer != Some(writer)
                || g.credential_fence
            {
                return Err(super::recovery_required());
            }
            g.ever_authenticated |= g.authenticated;
            g.authenticated = false;
            g.credential_mutation = None;
            g.credential_writer = None;
            index.save()
        })
    }
}
