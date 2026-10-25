//! Budgets shared by provider transport and native discovery.
use std::time::Duration;

// Native providers may finish asynchronous teardown after their input reaches EOF.
pub(super) const NATIVE_EOF_GRACE: Duration = Duration::from_secs(10);
