use super::Owner;
use std::{
    io,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

/// A blocked maintenance operation keeps its owner lock; shutdown never waits on it forever.
pub struct Guard {
    stop: Sender<()>,
    done: Receiver<()>,
}
pub(super) fn start(mut owner: Owner) -> io::Result<Guard> {
    let (stop, stopping) = mpsc::channel();
    let (finished, done) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("hive-raw-retention".into())
        .spawn(move || {
            let mut failures = 0_u64;
            loop {
                match stopping.recv_timeout(RETENTION_INTERVAL) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => (),
                }
                match owner.trim() {
                    Ok(true) => tracing::info!(name: "log.raw_trimmed",
                     bytes = super::TAIL_BYTES,
                     decision = "retain_tail_and_active_inode"),

                    Ok(false) => (),
                    Err(error) => {
                        failures = failures.saturating_add(1);
                        if failures.is_power_of_two() {
                            tracing::warn!(name: "log.raw_retention_failed",
                             count = failures,
                             error = &error as &(dyn std::error::Error + 'static),
                             decision = "retain_active_bytes");
                        }
                    }
                }
            }
            drop(owner);
            // A caller that already exhausted its wait budget needs no notification.
            let _ = finished.try_send(());
        })?;
    Ok(Guard { stop, done })
}
impl Guard {
    pub fn finish(self, timeout: Duration) -> bool {
        // Disconnect means the worker already exited; the completion channel is authoritative.
        let _ = self.stop.send(());
        self.done.recv_timeout(timeout).is_ok()
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.stop.send(());
    }
}

const RETENTION_INTERVAL: std::time::Duration = Duration::from_secs(1);
