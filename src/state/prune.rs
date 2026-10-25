//! Explicit removal of unreferenced, resolved retired credential namespaces.
use super::{Binding, GenerationState, Lock, LockIdentity, State, corrupt, journal};
use crate::{
    error::{Error, Result},
    runtime::{self, api::value as project},
    state::OperationKind,
};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use uuid::Uuid;

mod admission;
#[cfg(any(target_os = "macos", test))]
mod native;
mod queries;
mod retirement;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
    Prune,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    Pruned,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PruneReceipt {
    account_sessions_revoked: bool,
    checked_at: u64,
    generation: Uuid,
    local_credentials_removed: bool,
    operation: Operation,
    outcome: Outcome,
    profile: String,
}

impl PruneReceipt {
    fn validate(&self, name: &str, id: Uuid) -> Result<()> {
        if self.profile != name
            || self.generation != id
            || !self.local_credentials_removed
            || self.account_sessions_revoked
            || self.checked_at == 0
        {
            return Err(corrupt());
        }
        Ok(())
    }
}

impl State {
    pub fn prune(&self, name: &str, id: Uuid) -> Result<Value> {
        let _admission = self.admission()?;
        self.check_format()?;
        let dir = self.root.join("profiles").join(id.to_string());
        if super::recovery::exists(&dir)? {
            super::recovery::layout::audit_profile(&dir)?;
        }
        // Offline maintenance excludes the daemon's uncommitted spawn pins as
        // well as committed agents. Never create a second database writer.
        let _controller = runtime::maintenance_lock(&self.root)?;
        let binding = self.prune_preflight(name, id)?;
        let operation = self
            .operation(&binding, OperationKind::Prune)
            .map_err(|error| {
                if matches!(error, Error::RecoveryRequired { .. }) {
                    Error::recovery_required(PRUNE_OWNERSHIP_UNPROVEN).with_source(error)
                } else {
                    error
                }
            })?;
        let terminal = self.mark_pruning(&binding, id)?;
        let lock = LockIdentity::from_fd(operation.fd())?;
        let receipt =
            retirement::finish(&binding, name, &lock, terminal, || self.prune_native(id))?;
        self.update_index(|index| {
            let generation = index.generation_mut(id)?;
            if !matches!(
                generation.state,
                GenerationState::Pruning | GenerationState::Pruned
            ) || !generation.binding.same_identity(&binding)
            {
                return Err(corrupt());
            }
            generation.state = GenerationState::Pruned;
            generation.authenticated = false;
            generation.possibly_authenticated = false;
            index.save()
        })?;
        operation.finish()?;
        Ok(project(receipt))
    }
    fn prune_preflight(&self, name: &str, id: Uuid) -> Result<Binding> {
        let (mut binding, candidate_owner) = self.prune_candidate(name, id)?;
        if let Some(owner) = candidate_owner {
            if !owner.absent()? {
                return Err(Error::profile_busy(
                    "Repair still owns this candidate generation.",
                ));
            }
            self.update_index(|index| {
                let generation = index.generation_mut(id)?;
                if generation.state != GenerationState::Candidate
                    || generation.repair_owner.as_ref() != Some(&owner)
                {
                    return Err(corrupt());
                }
                generation.state = GenerationState::Retired;
                if !binding.ready {
                    // Registration-ready is durable before any native launch can
                    // start. An abandoned earlier allocation never held credentials.
                    self.validate_binding(&binding)?;
                    let dir = binding.directory.clone();
                    super::directory(&dir)?;
                    super::write_json(&dir.join("identity.json"), &binding)?;
                    for content in ["provider", "work"] {
                        super::directory(&dir.join(content))?;
                    }
                    let credential = Lock::acquire_durable(&dir.join("auth.lock"))?;
                    generation.credential_lock =
                        Some(super::LockIdentity::from_fd(credential.fd())?);
                    binding.ready = true;
                    generation.binding = binding.clone();
                    generation.state = GenerationState::Pruning;
                    generation.credential_namespace = None;
                }
                index.save()
            })?;
        }
        Ok(binding)
    }
    fn no_agent_references(&self, binding: &Binding, index: &super::profiles::Index) -> Result<()> {
        let path = self.root.join("runtime/runtime.sqlite");
        if !super::recovery::exists(&path)? {
            return Ok(());
        }
        super::check_path(&path, false)?;
        let conn = {
            let _admission = runtime::database_admission();
            Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| corrupt().with_source(e))?
        };
        runtime::validate_schema(&conn)?;
        let mut statement = conn
            .prepare(queries::NO_AGENT_REFERENCES)
            .map_err(|e| corrupt().with_source(e))?;
        let pins = statement
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| corrupt().with_source(e))?;
        let mut pinned = false;
        for pin in pins {
            let pin = pin.map_err(|e| corrupt().with_source(e))?;
            let id = Uuid::parse_str(&pin).map_err(|e| corrupt().with_source(e))?;
            if !index.generations.contains_key(&id) {
                return Err(corrupt());
            }
            pinned |= id == binding.id;
        }
        if pinned {
            return Err(Error::invalid_request(concat!(
                "Generation is pinned by saved agents; inspect their history. Native ",
                "conversations cannot be rebound.",
            )));
        }
        Ok(())
    }
}

impl State {
    fn mark_pruning(&self, binding: &Binding, id: Uuid) -> Result<bool> {
        self.update_index(|index| {
            if index.profiles.values().any(|b| b.id == id) {
                return Err(corrupt());
            }
            self.no_agent_references(binding, index)?;
            let namespace = index.generation(id)?.credential_namespace.clone();
            if let Some(service) = &namespace {
                if self.retired_namespaces()?.contains(service) {
                    return Err(corrupt());
                }
                index
                    .claude_namespaces(Some(id))
                    .try_for_each(|namespace| {
                        if namespace? == *service {
                            return Err(corrupt());
                        }
                        Ok(())
                    })?;
            }
            let generation = index.generation_mut(id)?;
            let terminal = generation.state == GenerationState::Pruned;
            if !matches!(
                generation.state,
                GenerationState::Retired | GenerationState::Pruning | GenerationState::Pruned
            ) || !generation.binding.same_identity(binding)
            {
                return Err(corrupt());
            }
            if !terminal {
                generation.state = GenerationState::Pruning;
                index.save()?;
            }
            Ok(terminal)
        })
    }
}

impl State {
    fn prune_candidate(
        &self,
        name: &str,
        id: Uuid,
    ) -> Result<(Binding, Option<super::ProcessIdentity>)> {
        let _metadata = self.metadata_lock()?;
        let mut index = self.read_index()?;
        let generation = index.generations.remove(&id).ok_or_else(corrupt)?;
        if generation.profile_name != name
            || !matches!(
                generation.state,
                GenerationState::Retired
                    | GenerationState::Pruning
                    | GenerationState::Pruned
                    | GenerationState::Candidate
            )
            || index.profiles.values().any(|b| b.id == id)
        {
            return Err(Error::invalid_request(
                "Only an inactive retired generation can be pruned.",
            ));
        }
        if generation.state == GenerationState::Candidate && generation.repair_owner.is_none() {
            return Err(corrupt());
        }
        Ok((
            generation.binding,
            if generation.state == GenerationState::Candidate {
                generation.repair_owner
            } else {
                None
            },
        ))
    }
}

impl PruneReceipt {
    fn load(path: &Path, name: &str, id: Uuid) -> Result<Option<Self>> {
        if super::recovery::exists(path)? {
            let receipt: PruneReceipt = super::read_json(path)?;
            receipt.validate(name, id)?;
            Ok(Some(receipt))
        } else {
            Ok(None)
        }
    }
}

impl State {
    fn prune_native(&self, id: Uuid) -> Result<()> {
        let generation = {
            let _metadata = self.metadata_lock()?;
            self.read_index()?.into_generation(id)?
        };
        #[cfg(target_os = "macos")]
        if generation.binding.profile.provider == crate::config::Provider::Claude
            && generation.binding.profile.method == crate::config::Method::Subscription
            && (generation.ever_authenticated || generation.possibly_authenticated)
        {
            native::remove(&generation)?;
        }
        #[cfg(not(target_os = "macos"))]
        let _ = generation;
        Ok(())
    }
}

const PRUNE_OWNERSHIP_UNPROVEN: &str = concat!(
    "This retired namespace cannot yet be pruned: its retained evidence does ",
    "not prove released ownership. Preserve it for inspection. Repair ",
    "restores availability in a new namespace but cannot establish missing ",
    "historical proof for this one.",
);

#[cfg(test)]
mod tests;
