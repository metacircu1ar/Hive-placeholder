//! Background embedding work with bounded adaptive batches.
use super::dispatch::join_error;
use super::{State, limits};
use crate::{
    Error, ErrorCode, Result,
    embedding::{self, Backfill, BackfillHost, Batch, BatchSizer, Scan},
};
use std::{
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};
mod progress;

#[cfg(test)]
mod tests;

struct ServiceHost<'a> {
    state: &'a State,
}
struct MeasuredBatch {
    inputs: usize,
    elapsed: Duration,
}
impl BackfillHost for ServiceHost<'_> {
    type Progress = MeasuredBatch;
    fn select(&mut self, scan: &mut Scan, limit: usize) -> Result<Batch> {
        // Selection can walk cached chunks; never hold the writer during it.
        // This snapshot is dropped before the cursor driver calls process.
        self.state.snapshot()?.index.embedding_jobs(scan, limit)
    }
    fn process(&mut self, batch: Batch) -> Result<Option<MeasuredBatch>> {
        let model = self
            .state
            .model
            .as_ref()
            .ok_or_else(Error::query_model_missing)?;
        // Both checks pair with PendingQueryGuard admission/release (SeqCst).
        if model.query_engine.is_none() && model.pending_queries.load(Ordering::SeqCst) > 0 {
            return Ok(None);
        }
        let mut engine = model.engine.lock()?;
        if model.query_engine.is_none() && model.pending_queries.load(Ordering::SeqCst) > 0 {
            return Ok(None);
        }
        let inputs = batch.jobs.len();
        let start = Instant::now();
        let completed = embedding::run_batch(&mut **engine, batch)?;
        let elapsed = start.elapsed();
        drop(engine);
        let mut comb = self.state.writer.lock()?;
        comb.apply_embeddings(completed)?;
        comb.embedding_worker_error = None;
        Ok(Some(MeasuredBatch { inputs, elapsed }))
    }
}

impl State {
    #[cfg(test)]
    pub(super) fn backfill_batch(&self, scan: &mut Scan) -> Result<bool> {
        let mut backfill = Backfill::from_scan(std::mem::take(scan));
        let result = backfill.step(&mut ServiceHost { state: self }, embedding::BATCH_SIZE);
        *scan = backfill.into_scan();
        result.map(|work| work.is_some())
    }

    pub(super) async fn backfill(self: Arc<Self>) {
        let mut backfill = Backfill::default();
        let mut sizer = BatchSizer::default();
        let mut progress = progress::Progress::new();
        loop {
            let limit = sizer.limit();
            let state = self.clone();
            let result = tokio::task::spawn_blocking(move || {
                let result =
                    state.record_backfill(backfill.step(&mut ServiceHost { state: &state }, limit));
                (backfill, result)
            })
            .await;
            let (next, result) = match result {
                Ok(result) => result,
                Err(cause) => {
                    self.health.error_or(join_error(cause)).log_diagnostic();
                    return;
                }
            };
            backfill = next;
            progress.record(&result, backfill.caught_up());
            if !self.finish_backfill(result, &mut sizer).await {
                return;
            }
        }
    }

    async fn finish_backfill(
        &self,
        result: Result<Option<MeasuredBatch>>,
        sizer: &mut BatchSizer,
    ) -> bool {
        match result {
            Ok(Some(work)) => sizer.record(work.inputs, work.elapsed),
            Ok(None) => {
                tokio::select! {()=self.wake.notified()=>{},
                ()=tokio::time::sleep(limits::BACKFILL_IDLE)=>{}}
                return true;
            }
            Err(error) => {
                error.log_diagnostic();
                if error.code == ErrorCode::RecoveryRequired || self.health.check().is_err() {
                    tracing::info!(name: "comb.backfill_stopped", decision = "recovery_required");
                    return false;
                }
                tracing::warn!(name: "comb.backfill_retry",
                     code = error.code.as_str(),
                     duration_ms = limits::BACKFILL_RETRY.as_millis(),
                     decision = "backoff");
                // New writes must not bypass a transient endpoint's backoff.
                // Notify retains its permit until the subsequent idle pass.
                tokio::time::sleep(limits::BACKFILL_RETRY).await;
            }
        }
        tokio::task::yield_now().await;
        true
    }

    fn record_backfill<T>(&self, result: Result<T>) -> Result<T> {
        if let Err(error) = &result {
            if error.code == crate::ErrorCode::RecoveryRequired {
                return result;
            }
            match self.writer.lock() {
                Ok(mut writer) => writer.embedding_worker_error = Some(error.clone()),
                Err(secondary) => {
                    tracing::warn!(name: "comb.worker_error_unrecorded",
                         primary = error as &(dyn std::error::Error + 'static),
                         secondary = &secondary as &(dyn std::error::Error + 'static),
                         decision = "preserve_primary_failure");
                }
            }
        }
        result
    }
}
