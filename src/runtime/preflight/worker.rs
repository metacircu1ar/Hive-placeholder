//! Worker launch proofs bound to the saved conversation identity.
use super::Leases;
use crate::runtime::steering_canary as steering;
use crate::{
    config::Profile,
    error::{Error, Result},
    runtime::domain::Agent,
    state::{RuntimeLease, State},
};
use std::{path::PathBuf, sync::Arc};

// Bind external proofs to the persisted conversation identity. Lifecycle is
// independently reloaded and checked immediately before consuming the proof.
#[derive(Clone, PartialEq, Eq)]
pub(in crate::runtime) struct Identity {
    id: String,
    auth: String,
    generation: uuid::Uuid,
    profile: Profile,
    provider: Option<String>,
    execution: Option<String>,
    thread: Option<String>,
    clean: bool,
    input_attempted: bool,
    cwd: PathBuf,
}
impl From<&Agent> for Identity {
    fn from(agent: &Agent) -> Self {
        Self {
            id: agent.id.clone(),
            auth: agent.auth.clone(),
            generation: agent.auth_generation,
            profile: agent.profile.clone(),
            provider: agent.provider_id().map(str::to_owned),
            execution: agent.execution_id.clone(),
            thread: agent.thread_id.clone(),
            clean: agent.conversation_clean,
            input_attempted: agent.native_input_attempted,
            cwd: agent.cwd.clone(),
        }
    }
}
pub(in crate::runtime) struct WorkerInput {
    pub agent: Agent,
    pub launcher: Arc<crate::launch::LauncherCache>,
    pub state: State,
    pub profile: Result<Profile>,
    pub binary: Result<PathBuf>,
}
pub(in crate::runtime) struct WorkerProof {
    pub identity: Identity,
    pub checked: Result<ExecutionProfile>,
}
pub(in crate::runtime) struct ExecutionProfile {
    pub lease: Arc<RuntimeLease>,
    pub dir: PathBuf,
    pub binary: PathBuf,
    pub steering: Option<steering::Candidate>,
    pub credential: crate::auth::Credential,
    pub compatibility: crate::provider_support::Evidence,
}
impl WorkerInput {
    pub(super) fn resolve(self, leases: &Leases) -> WorkerProof {
        let identity = Identity::from(&self.agent);
        let checked = self.check(leases);
        WorkerProof { identity, checked }
    }
    fn check(self, leases: &Leases) -> Result<ExecutionProfile> {
        self.launcher.check_workspace(&self.agent.cwd)?;
        // Pin the command to the same target whose identity/catalog/proof we
        // validate. Retargeting an installation symlink affects the next
        // preflight, never this already validated launch.
        let binary = self.binary?.canonicalize().map_err(Error::state_io_from)?;
        let compatibility = crate::provider_support::observe(
            &self.launcher,
            &self.state,
            &binary,
            self.agent.profile.provider,
        )?;
        if self.agent.branched_from_agent_id.is_some() {
            compatibility.require_branching()?;
        }
        let profile = self.profile?;
        if profile != self.agent.profile {
            return Err(Error::binding_conflict(
                "Agent's saved auth profile changed.",
            ));
        }
        let binding =
            self.state
                .generation(&self.agent.auth, &profile, self.agent.auth_generation)?;
        if !self.agent.conversation_clean {
            self.state
                .check_conversation(&binding, self.agent.provider_id())?;
        }
        let lease = leases.checkout(&self.state, &binding)?;
        let credential = leases.keys.resolve(&self.state, &binding)?;
        let steering = if profile.provider == crate::config::Provider::Claude {
            // Executable/auth ownership remains mandatory. Optional capability
            // evidence may fail without denying ordinary Claude execution.
            crate::runtime::provider::catalogs::BinaryIdentity::read(&binary)?;
            Some(
                steering::load(&self.state.root, &binary)
                    .inspect_err(|error| {
                        crate::observability::observation_failed("preflight_steering_proof", error);
                    })
                    .unwrap_or_else(|error| steering::Candidate::invalid(&error)),
            )
        } else {
            None
        };
        credential.verify_source(&self.state, &binding)?;
        Ok(ExecutionProfile {
            credential,
            steering,
            lease,
            dir: binding.directory.clone(),
            binary,
            compatibility,
        })
    }
}
