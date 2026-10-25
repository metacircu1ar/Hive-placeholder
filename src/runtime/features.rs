//! Compiled capabilities are admission policy, never persisted lifecycle state.
use super::domain::Agent;
use crate::error::{Error, Result};

// `barrier-delay` builds are for measurement only. Their version names the
// feature, so the release build driver and the installer refuse them as parts.
const BUILD: (bool, bool) = (cfg!(feature = "comb"), cfg!(feature = "barrier-delay"));
pub(crate) const COMPILED: &[&str] = match BUILD {
    (true, false) => &["comb"],
    (true, true) => &["comb", "barrier-delay"],
    (false, false) => &[],
    (false, true) => &["barrier-delay"],
};
pub(crate) const VERSION: &str = match BUILD {
    (true, false) => concat!(env!("CARGO_PKG_VERSION"), " (comb)"),
    (true, true) => concat!(env!("CARGO_PKG_VERSION"), " (comb, barrier-delay)"),
    (false, false) => concat!(env!("CARGO_PKG_VERSION"), " (core)"),
    (false, true) => concat!(env!("CARGO_PKG_VERSION"), " (core, barrier-delay)"),
};
pub(crate) fn available(agent: &Agent) -> bool {
    cfg!(feature = "comb") || agent.comb.is_none()
}
pub(crate) fn required_build(agent: &Agent) -> Option<super::api::RequiredBuild> {
    (!available(agent) && !agent.terminal()).then_some(super::api::RequiredBuild::Comb)
}
pub(crate) fn require(agent: &Agent) -> Result<()> {
    if available(agent) {
        Ok(())
    } else {
        Err(unavailable())
    }
}
pub(crate) fn unavailable() -> Error {
    Error::unsupported_operation(
        "This saved agent requires an Comb-capable Hive build; its input is \
            retained. Install Hive with the comb feature to proceed.",
    )
}

// One admission predicate for core builds. The generated column is unindexed;
// ready queries use agents_ready. Default builds add no filter or JSON scan.
#[cfg(feature = "comb")]
macro_rules! executable {
    ($column:literal) => {
        ""
    };
}
#[cfg(not(feature = "comb"))]
macro_rules! executable {
    ($column:literal) => {
        concat!(" AND ", $column, " = 0 ")
    };
}
pub(in crate::runtime) use executable;
