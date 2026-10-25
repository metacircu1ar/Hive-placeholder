//! Native branching admission before accepting a durable child intent.
use crate::{config::Provider, error::Result, launch::LauncherCache, state::State};
use std::path::Path;
pub(in crate::runtime) fn verify_binary(
    launcher: &LauncherCache,
    state: &State,
    binary: &Path,
    provider: Provider,
) -> Result<()> {
    let evidence = crate::provider_support::observe(launcher, state, binary, provider)?;
    evidence.require_branching()?;
    evidence.check()
}
