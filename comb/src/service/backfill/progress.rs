use super::{Instant, MeasuredBatch, Result};
use std::time::Duration;

pub(super) struct Progress {
    inputs: usize,
    last: Instant,
    idle: bool,
}
impl Progress {
    pub fn new() -> Self {
        Self {
            inputs: 0,
            last: Instant::now(),
            idle: false,
        }
    }
    pub fn record(&mut self, result: &Result<Option<MeasuredBatch>>, empty: bool) {
        match result {
            Ok(Some(work)) => {
                self.idle = false;
                self.inputs = self.inputs.saturating_add(work.inputs);
                if self.last.elapsed() >= PROGRESS_INTERVAL {
                    tracing::info!(name: "comb.backfill_progress",
                         count = self.inputs,
                         elapsed_ms = self.last.elapsed().as_millis());
                    self.last = Instant::now();
                }
            }
            Ok(None) if empty && !self.idle => {
                self.idle = true;
                tracing::info!(name: "comb.backfill_caught_up", count = self.inputs);
            }
            // Yielding to queries is not an empty work queue.
            Ok(None) | Err(_) => (),
        }
    }
}
impl Drop for Progress {
    fn drop(&mut self) {
        tracing::info!(name: "comb.backfill_finished",
             count = self.inputs,
             decision = "worker_exited");
    }
}

const PROGRESS_INTERVAL: std::time::Duration = Duration::from_secs(10);
