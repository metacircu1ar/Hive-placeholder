//! Durable per-login load, observed within the selecting writer transaction.
mod queries;
use super::Queries;
use crate::error::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AuthLoad {
    pub bound_agents: u64,
    pub active_turns: u64,
}

impl Queries {
    pub(crate) fn auth_load(&self, profile: &str) -> Result<AuthLoad> {
        let (bound, active): (i64, i64) =
            self.sql()
                .cached()
                .query_row(queries::AUTH_LOAD_SELECT, [profile], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?;
        Ok(AuthLoad {
            bound_agents: u64::try_from(bound).map_err(super::numeric)?,
            active_turns: u64::try_from(active).map_err(super::numeric)?,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Fixtures stop immediately on store setup or query failure.
mod tests;
