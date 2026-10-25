//! Shared filesystem context prepares member bindings without entering the writer.
use crate::{
    config::{Config, Provider},
    error::{Error, Result},
    runtime::{
        Core,
        binaries::ResolvedBinaries,
        provider::{catalogs::BindingInput, protocol::Model},
    },
    state::{Binding, Lock, RuntimeLease, State},
};
use std::{path::PathBuf, sync::Arc};

pub(super) struct FsContext {
    pub state: State,
    pub config: Config,
    pub config_dir: PathBuf,
    binaries: ResolvedBinaries,
    daemon_lock: Arc<Lock>,
    pub(super) launcher: Arc<crate::launch::LauncherCache>,
    keys: Arc<crate::auth::Keys>,
}
pub(in crate::runtime) struct MemberFacts {
    pub auth: String,
    pub binding: Result<Binding>,
    pub catalog: Option<Result<Vec<Model>>>,
    pub lease: Option<Arc<RuntimeLease>>,
    pub credential: Option<crate::auth::Credential>,
}
impl MemberFacts {
    pub(in crate::runtime) fn fresh(&self) -> bool {
        self.credential
            .as_ref()
            .is_none_or(crate::auth::Credential::fresh)
    }
    pub(in crate::runtime) fn authority(&self) -> Option<&crate::state::KeyAuthority> {
        self.credential
            .as_ref()
            .and_then(crate::auth::Credential::authority)
    }
}
#[derive(Clone, Copy)]
pub(super) enum MemberPurpose {
    Spawn { generation: Option<uuid::Uuid> },
    LoginSwitch,
}
impl MemberPurpose {
    fn generation(self) -> Option<uuid::Uuid> {
        match self {
            Self::Spawn { generation } => generation,
            Self::LoginSwitch => None,
        }
    }
}
impl Core {
    pub(in crate::runtime::preflight) fn fs_context(&self) -> FsContext {
        FsContext {
            state: self.state().clone(),
            config: self.config().clone(),
            config_dir: self.config_dir().clone(),
            binaries: self.binaries().clone(),
            daemon_lock: self.daemon_lock().clone(),
            launcher: self.launcher().clone(),
            keys: self.leases().keys.clone(),
        }
    }
}
impl FsContext {
    pub fn member(
        &self,
        auth: String,
        purpose: MemberPurpose,
    ) -> (MemberFacts, Option<BindingInput>) {
        let checked = self.binding(&auth, purpose);
        let (binding, lease, credential) = match checked {
            Ok((binding, lease, credential)) => (Ok(binding), Some(lease), Some(credential)),
            Err(error) => (Err(error), None, None),
        };
        let discovery =
            binding
                .as_ref()
                .ok()
                .zip(credential.as_ref())
                .map(|(binding, credential)| {
                    self.binary(binding.profile.provider, purpose.generation().is_some())
                        .map(|binary| BindingInput {
                            auth: auth.clone(),
                            profile: binding.profile.clone(),
                            generation: binding.id,
                            state: self.state.clone(),
                            binary,
                            daemon_lock: self.daemon_lock.clone(),
                            launcher: self.launcher.clone(),
                            credential: Some(credential.clone()),
                            keys: self.keys.clone(),
                        })
                });
        let (catalog, discovery) = match discovery {
            Some(Ok(input)) => (None, Some(input)),
            Some(Err(error)) => (Some(Err(error)), None),
            None => (None, None),
        };
        (
            MemberFacts {
                auth,
                binding,
                catalog,
                lease,
                credential,
            },
            discovery,
        )
    }
    fn binding(
        &self,
        auth: &str,
        purpose: MemberPurpose,
    ) -> Result<(Binding, Arc<RuntimeLease>, crate::auth::Credential)> {
        let profile = self.config.profile(auth)?;
        let binding = {
            if let Some(generation) = purpose.generation() {
                self.state.generation(auth, profile, generation)?
            } else {
                let _metadata = self.state.metadata_lock()?;
                self.state.selected_generation(auth, profile)?
            }
        };
        if !binding.ready {
            return Err(Error::login_required("Finish profile login first."));
        }
        let lease = Arc::new(self.state.runtime_lease(&binding)?);
        let credential = self.keys.resolve(&self.state, &binding)?;
        Ok((binding, lease, credential))
    }
    fn binary(&self, provider: Provider, branching: bool) -> Result<PathBuf> {
        let binary = self.binaries.resolve(provider)?;
        if branching {
            let binary = binary.canonicalize().map_err(Error::state_io_from)?;
            crate::runtime::branching::verify_binary(
                &self.launcher,
                &self.state,
                &binary,
                provider,
            )?;
            return Ok(binary);
        }
        Ok(binary)
    }
}
