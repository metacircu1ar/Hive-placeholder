//! Bounded executable observations; no credential access or inference turns.
use super::{Observation, classify};
use crate::runtime::ProviderBinaryIdentity;
use crate::{
    config::Provider,
    error::{Error, Result},
    launch::LauncherCache,
    process::{self, Cancellation},
    runtime::provider_binary_identity,
    state::State,
};
use std::{
    collections::VecDeque,
    path::Path,
    sync::{Mutex, OnceLock},
    time::Duration,
};

#[derive(Clone, Debug)]
pub(crate) struct Evidence {
    pub observation: Observation,
    pub identity: ProviderBinaryIdentity,
    codex: Option<super::codex::Capabilities>,
}
impl Evidence {
    pub(crate) fn require_branching(&self) -> Result<()> {
        match self.observation.registry.provider {
            Provider::Codex => self
                .codex
                .as_ref()
                .ok_or_else(|| {
                    Error::unsupported_provider(
                        "Codex protocol capabilities have not been verified.",
                    )
                })?
                .require_branching(),
            Provider::Claude => Ok(()),
        }
    }

    pub(crate) fn check(&self) -> Result<()> {
        if !self.identity.unchanged() {
            return Err(Error::provider_unavailable(
                "Provider executable changed after compatibility verification.",
            ));
        }
        self.observation.admit()
    }
}
const CACHE_ENTRIES: usize = 64;
static CACHE: OnceLock<Mutex<VecDeque<Evidence>>> = OnceLock::new();

pub(crate) fn observe(
    launcher: &LauncherCache,
    state: &State,
    binary: &Path,
    provider: Provider,
) -> Result<Evidence> {
    let pinned = binary.canonicalize().map_err(Error::state_io_from)?;
    let binary = pinned.as_path();
    let identity = provider_binary_identity(binary)?;
    let cache = CACHE.get_or_init(Mutex::default);
    {
        let entries = cache
            .lock()
            .map_err(|_| Error::recovery_required("Provider compatibility cache is poisoned."))?;
        if let Some(evidence) = entries
            .iter()
            .find(|e| e.identity == identity && e.observation.registry.provider == provider)
        {
            evidence.check()?;
            return Ok(evidence.clone());
        }
    }
    let mut operation = state.metadata_probe()?;
    let result = (|| {
        let mut command = operation.version_command(binary, provider)?;
        process::version_probe(
            launcher,
            &mut command,
            &mut operation,
            PROBE_TIMEOUT,
            Cancellation::default(),
            None,
            |output, operation| {
                if !output.status.success() {
                    return Err(Error::provider_failed("Provider version probe failed."));
                }
                operation.verify_probe_ownership()?;
                let version = crate::auth::parse_version(provider, &output.stdout)?;
                let observation = classify(provider, &version)?;
                let evidence = Evidence {
                    identity,
                    observation,
                    codex: None,
                };
                evidence.check()?;
                Ok(evidence)
            },
        )
    })();
    let finished = operation.finish();
    state.sweep_metadata_probes();
    let mut evidence = crate::observability::settle(result, finished, "provider_version_probe")?;
    if provider == Provider::Codex {
        evidence.codex = Some(super::codex::probe(
            launcher,
            state,
            binary,
            &evidence.identity,
        )?);
        evidence.check()?;
    }
    let mut entries = cache
        .lock()
        .map_err(|_| Error::recovery_required("Provider compatibility cache is poisoned."))?;
    entries
        .retain(|e| e.identity != evidence.identity || e.observation.registry.provider != provider);
    entries.push_back(evidence.clone());
    while entries.len() > CACHE_ENTRIES {
        entries.pop_front();
    }
    Ok(evidence)
}

const PROBE_TIMEOUT: std::time::Duration = Duration::from_secs(15);
