use std::time::Duration;

pub(super) const CAPABILITIES: Duration = Duration::from_secs(10);
pub(super) const REQUEST: Duration = Duration::from_secs(60);
pub(super) const SHUTDOWN: Duration = Duration::from_millis(500);

pub(super) const WORKER_THREADS: usize = 2;
