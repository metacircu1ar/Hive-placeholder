use std::time::{Duration, Instant};

pub const TEST_TIMEOUT: Duration = Duration::from_secs(15);
pub const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
pub const POLL_INTERVAL: Duration = Duration::from_millis(20);

pub fn wait_until<T>(
    timeout: Duration,
    interval: Duration,
    description: &str,
    predicate: impl FnMut() -> Option<T>,
) -> T {
    try_wait_until(timeout, interval, predicate)
        .unwrap_or_else(|| panic!("timed out waiting for {description}"))
}

pub fn try_wait_until<T>(
    timeout: Duration,
    interval: Duration,
    mut predicate: impl FnMut() -> Option<T>,
) -> Option<T> {
    let started = Instant::now();
    loop {
        if let Some(value) = predicate() {
            return Some(value);
        }
        if started.elapsed() >= timeout {
            return None;
        }
        std::thread::sleep(interval);
    }
}
