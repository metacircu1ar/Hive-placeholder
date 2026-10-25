//! Create and settle isolated credential generation replacements.
use super::{
    Binding, Error, Generation, GenerationState, Lock, Repair, Result, State,
    claude_keychain_account, corrupt,
};
use crate::{
    config::{Profile, Provider},
    state,
};
use std::collections::HashSet;
use uuid::Uuid;

impl State {
    pub fn begin_repair(&self, name: &str, profile: &Profile) -> Result<Repair> {
        let admission = self.admission()?;
        let keychain_account = (profile.provider == Provider::Claude)
            .then(claude_keychain_account)
            .transpose()?;
        self.update_index(|index| {
            let old = index
                .profiles
                .get(name)
                .ok_or(Error::login_required(state::REGISTER_PROFILE_FIRST))?;
            if !old.ready {
                return Err(Error::registration_incomplete(
                    state::REGISTRATION_INCOMPLETE,
                ));
            }
            if old.profile != *profile {
                return Err(corrupt());
            }
            super::recovery::layout::profile(&old.directory)?;
            let expected = old.id;
            let mut namespaces: HashSet<_> =
                index.claude_namespaces(None).collect::<Result<_>>()?;
            if profile.provider == Provider::Claude {
                namespaces.extend(self.retired_namespaces()?);
            }
            let (binding, namespace) = self.pending_binding(profile, &namespaces)?;
            index.generations.insert(
                binding.id,
                candidate_generation(&binding, name, expected, namespace, keychain_account)?,
            );
            index.save()?;
            let dir = binding.directory.clone();
            Self::initialize_binding(&binding)?;
            let mut binding = binding;
            binding.ready = true;
            let credential = Lock::acquire_durable(&dir.join("auth.lock"))?;
            index.generation_mut(binding.id)?.credential_lock =
                Some(super::LockIdentity::from_fd(credential.fd())?);
            index.generation_mut(binding.id)?.binding = binding.clone();
            index.save()?;
            Ok(Repair {
                _admission: admission,
                binding,
                expected,
                name: name.into(),
            })
        })
    }

    pub fn finish_repair(
        &self,
        repair: &Repair,
        authenticated: bool,
        account: Option<String>,
    ) -> Result<bool> {
        let _admission = self.admission()?;
        self.check_format()?;
        let credential = authenticated
            .then(|| Lock::acquire(&repair.binding.directory.join("auth.lock")))
            .transpose()?;
        if let Some(lock) = &credential {
            self.verify_credential(&repair.binding, lock)?;
            super::recovery::ledger::check_writers(&repair.binding.directory, repair.binding.id)?;
        }
        self.update_index(|index| {
            let current = index.profiles.get(&repair.name).ok_or_else(corrupt)?.id;
            let selected = authenticated && current == repair.expected;
            let generation = index.generation_mut(repair.binding.id)?;
            if generation.state != GenerationState::Candidate
                || generation.replaces != Some(repair.expected)
            {
                return Err(corrupt());
            }
            generation.state = if selected {
                GenerationState::Active
            } else {
                GenerationState::Retired
            };
            generation.authenticated |= authenticated;
            generation.ever_authenticated |= generation.authenticated;
            generation.possibly_authenticated |= authenticated;
            if account.is_some() {
                generation.account_identity = account;
            }
            if selected {
                index.generation_mut(current)?.state = GenerationState::Retired;
                index
                    .profiles
                    .insert(repair.name.clone(), repair.binding.clone());
            }
            index.save()?;
            Ok(selected)
        })
    }

    pub(crate) fn retire_repair(&self, repair: &Repair) -> Result<()> {
        self.update_index(|index| {
            if index.profiles.values().any(|b| b.id == repair.binding.id) {
                return Err(corrupt());
            }
            let g = index.generation_mut(repair.binding.id)?;
            if g.replaces != Some(repair.expected)
                || !matches!(
                    g.state,
                    GenerationState::Candidate | GenerationState::Retired
                )
            {
                return Err(corrupt());
            }
            if g.state == GenerationState::Candidate {
                g.state = GenerationState::Retired;
                index.save()?;
            }
            Ok(())
        })
    }
}

fn candidate_generation(
    binding: &Binding,
    name: &str,
    expected: Uuid,
    namespace: Option<String>,
    keychain_account: Option<String>,
) -> Result<Generation> {
    Ok(Generation {
        state: GenerationState::Candidate,
        replaces: Some(expected),
        credential_namespace: namespace,
        repair_owner: Some(super::ProcessIdentity::current()?),
        ..Generation::new(binding.clone(), name, keychain_account)
    })
}
