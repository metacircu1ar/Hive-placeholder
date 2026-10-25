//! One cursor driver for service and offline embedding work.
use super::{BATCH_SIZE, Batch, Embedder, Scan, limits, run_batch};
use crate::{Comb, Result, convert};
use std::time::Duration;

pub(crate) trait BackfillHost {
    type Progress;
    fn select(&mut self, scan: &mut Scan, limit: usize) -> Result<Batch>;
    fn process(&mut self, batch: Batch) -> Result<Option<Self::Progress>>;
}

/// Bounded pending-work cursor shared by online and offline backfill.
#[derive(Default)]
pub struct Backfill {
    scan: Scan,
    empty: bool,
}
impl Backfill {
    pub(crate) fn step<H: BackfillHost>(
        &mut self,
        host: &mut H,
        limit: usize,
    ) -> Result<Option<H::Progress>> {
        let batch = host.select(&mut self.scan, limit)?;
        self.empty = batch.jobs.is_empty();
        if self.empty {
            return Ok(None);
        }
        let after = self.scan.after;
        // Retrying or yielding must revisit this batch, not the embedded prefix.
        self.scan.retry(&batch);
        let progress = host.process(batch)?;
        if progress.is_some() {
            self.scan.after = after;
        }
        Ok(progress)
    }

    pub(crate) fn caught_up(&self) -> bool {
        self.empty
    }

    /// Infer and publish up to 32 pending inputs; false means this pass is empty.
    pub fn offline_step(
        &mut self,
        comb: &mut Comb,
        embedder: &mut (impl Embedder + ?Sized),
    ) -> Result<bool> {
        self.step(&mut Offline { comb, embedder }, BATCH_SIZE)
            .map(|work| work.is_some())
    }

    #[cfg(test)]
    pub(crate) fn from_scan(scan: Scan) -> Self {
        Self { scan, empty: false }
    }
    #[cfg(test)]
    pub(crate) fn into_scan(self) -> Scan {
        self.scan
    }
}

struct Offline<'a, E: ?Sized> {
    comb: &'a mut Comb,
    embedder: &'a mut E,
}
impl<E: Embedder + ?Sized> BackfillHost for Offline<'_, E> {
    type Progress = ();
    fn select(&mut self, scan: &mut Scan, limit: usize) -> Result<Batch> {
        self.comb.index()?.embedding_jobs(scan, limit)
    }
    fn process(&mut self, batch: Batch) -> Result<Option<()>> {
        let completed = run_batch(self.embedder, batch)?;
        self.comb.apply_embeddings(completed)?;
        Ok(Some(()))
    }
}

#[derive(Default)]
pub(crate) struct BatchSizer {
    per_input: Option<f64>,
}
impl BatchSizer {
    pub(crate) fn limit(&self) -> usize {
        // Separate HTTP connections can still share a server's GPU/queue.
        // Start conservatively for every backend, then target ~200 ms from
        // measured inference cost. One input is never preempted.
        self.per_input
            .map_or(limits::BACKFILL_MIN_BATCH, |seconds| {
                convert::f64_usize_saturating(
                    (limits::BACKFILL_TARGET_SECONDS
                        / seconds.max(limits::BACKFILL_TIME_FLOOR_SECONDS))
                    .floor()
                    .clamp(
                        convert::usize_f64(limits::BACKFILL_MIN_BATCH),
                        convert::usize_f64(BATCH_SIZE),
                    ),
                )
            })
    }
    pub(crate) fn record(&mut self, inputs: usize, elapsed: Duration) {
        let sample = elapsed.as_secs_f64() / convert::usize_f64(inputs);
        self.per_input = Some(self.per_input.map_or(sample, |old| {
            old * limits::BACKFILL_OLD_WEIGHT + sample * limits::BACKFILL_SAMPLE_WEIGHT
        }));
    }
}

#[cfg(test)]
mod tests;
