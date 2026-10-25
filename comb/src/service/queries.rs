//! One bounded query owner coalesces fan-out searches without occupying the writer.
use super::observations::{Counters, Loss};
use super::{Model, State, join_error};
use crate::{Error, Result, embedding, locks::Health};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};

use super::limits::{
    QUERY_GATHER as GATHER, QUERY_SLOTS as CAPACITY, QUERY_START as START_TIMEOUT,
};
pub(super) struct Queue {
    sender: mpsc::Sender<Job>,
    health: Arc<Health>,
    pub(super) slots: Arc<Semaphore>,
}
pub(super) struct Job {
    text: String,
    deadline: tokio::time::Instant,
    started: oneshot::Sender<()>,
    reply: oneshot::Sender<Result<embedding::Query>>,
    _slot: OwnedSemaphorePermit,
    waiting: Option<PendingQueryGuard>,
}
struct PendingQueryGuard(Arc<AtomicUsize>);
impl PendingQueryGuard {
    fn new(pending: Arc<AtomicUsize>) -> Self {
        // SeqCst pairs admission/release with both shared-engine priority checks.
        pending.fetch_add(1, Ordering::SeqCst);
        Self(pending)
    }
}
impl Drop for PendingQueryGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
struct AwaitingQuery {
    deadline: tokio::time::Instant,
    started: oneshot::Receiver<()>,
    result: oneshot::Receiver<Result<embedding::Query>>,
}
impl AwaitingQuery {
    async fn wait(self, health: &Health) -> Result<embedding::Query> {
        tokio::time::timeout_at(self.deadline, self.started)
            .await
            .map_err(|cause| health.error_or(Error::embeddings_busy().caused_by(cause)))?
            .map_err(|cause| health.error_or(Error::embeddings_busy().caused_by(cause)))?;
        self.result
            .await
            .map_err(|cause| health.error_or(Error::query_worker_stopped().caused_by(cause)))?
            .map_err(|error| health.error_or(error))
    }
}
impl Queue {
    pub(super) fn new(health: Arc<Health>) -> (Self, mpsc::Receiver<Job>) {
        let (sender, receiver) = mpsc::channel(CAPACITY);
        (
            Self {
                sender,
                health,
                slots: Arc::new(Semaphore::new(CAPACITY)),
            },
            receiver,
        )
    }
    pub(super) async fn query(&self, text: &str, model: &Model) -> Result<embedding::Query> {
        self.enqueue(text, model)?.wait(&self.health).await
    }
    fn enqueue(&self, text: &str, model: &Model) -> Result<AwaitingQuery> {
        self.health.check()?;
        embedding::validate_query(text)?;
        let slot = self.slots.clone().try_acquire_owned().map_err(|cause| {
            self.health
                .error_or(Error::embeddings_busy().caused_by(cause))
        })?;
        let deadline = tokio::time::Instant::now() + START_TIMEOUT;
        let (started, start) = oneshot::channel();
        let (reply, result) = oneshot::channel();
        let waiting = PendingQueryGuard::new(model.pending_queries.clone());
        self.sender
            .try_send(Job {
                text: text.into(),
                deadline,
                started,
                reply,
                _slot: slot,
                waiting: Some(waiting),
            })
            .map_err(|cause| {
                self.health.error_or(
                    Error::embeddings_busy().caused_by(crate::error::Cause::channel(cause)),
                )
            })?;
        Ok(AwaitingQuery {
            deadline,
            started: start,
            result,
        })
    }
}
pub(super) async fn serve(state: Arc<State>, mut receiver: mpsc::Receiver<Job>) {
    while let Some(first) = receiver.recv().await {
        let batch = gather(first, &mut receiver).await;
        let owner = state.clone();
        let completed = tokio::task::spawn_blocking(move || run(&owner, batch)).await;
        let failed = completed.is_err();
        if let Err(cause) = completed {
            if cause.is_panic() {
                state.health.fence();
            }
            state.health.error_or(join_error(cause)).log_diagnostic();
        }
        if let Err(error) = state.health.check() {
            receiver.close();
            while let Some(job) = receiver.recv().await {
                reject(&state.observations, std::iter::once(job), &error);
            }
            break;
        }
        if failed {
            break;
        }
        state.wake.notify_one();
    }
}
async fn gather(first: Job, receiver: &mut mpsc::Receiver<Job>) -> Vec<Job> {
    let mut batch = vec![first];
    // Short bounded gathering helps independently arriving socket requests.
    tokio::time::sleep(GATHER).await;
    while batch.len() < embedding::BATCH_SIZE {
        let Ok(job) = receiver.try_recv() else {
            break;
        };
        batch.push(job);
    }
    batch
}
fn run(state: &State, mut jobs: Vec<Job>) {
    // Response owners outlive the inner engine guard. During inference unwind,
    // the guard fences health before dropping a sender can wake its caller.
    let mut started = Vec::new();
    if let Err(error) = run_locked(state, &mut jobs, &mut started) {
        reject(&state.observations, jobs, &error);
    }
}
struct ReplySlot {
    sender: oneshot::Sender<Result<embedding::Query>>,
    _permit: OwnedSemaphorePermit,
}
struct RunningQuery {
    text: String,
    reply: ReplySlot,
}
fn run_locked(state: &State, jobs: &mut Vec<Job>, started: &mut Vec<RunningQuery>) -> Result<()> {
    let Some(model) = &state.model else {
        return Ok(());
    };
    let mut engine = model
        .query_engine
        .as_ref()
        .unwrap_or(&model.engine)
        .lock()?;
    start_jobs(&state.observations, jobs, started);
    if started.is_empty() {
        return Ok(());
    }
    let texts = started
        .iter()
        .map(|job| job.text.clone())
        .collect::<Vec<_>>();
    let results = embedding::run_queries(&mut **engine, model.lease.clone(), &texts);
    complete(&state.observations, started, results);
    Ok(())
}
fn start_jobs(counters: &Counters, jobs: &mut Vec<Job>, started: &mut Vec<RunningQuery>) {
    // Check after engine acquisition, and release every waiting guard before any ack.
    jobs.retain(|job| {
        let keep = !job.reply.is_closed() && tokio::time::Instant::now() < job.deadline;
        if !keep {
            counters.lost(Loss::QueryReply, None);
        }
        keep
    });
    for job in &mut *jobs {
        job.waiting.take();
    }
    for job in jobs.drain(..) {
        let Job {
            started: ack,
            text,
            reply,
            _slot: slot,
            ..
        } = job;
        if ack.send(()).is_ok() && !reply.is_closed() {
            started.push(RunningQuery {
                text,
                reply: ReplySlot {
                    sender: reply,
                    _permit: slot,
                },
            });
        } else {
            counters.lost(Loss::QueryReply, None);
        }
    }
}
fn complete(
    counters: &Counters,
    started: &mut Vec<RunningQuery>,
    results: Result<Vec<Result<embedding::Query>>>,
) {
    match results {
        Ok(results) => {
            for (job, result) in started.drain(..).zip(results) {
                let ReplySlot { sender, _permit } = job.reply;
                observe_reply(counters, sender.send(result));
            }
        }
        Err(error) => {
            for job in started.drain(..) {
                let ReplySlot { sender, _permit } = job.reply;
                observe_reply(counters, sender.send(Err(error.clone())));
            }
        }
    }
}
fn reject(counters: &Counters, jobs: impl IntoIterator<Item = Job>, error: &Error) {
    for job in jobs {
        let Job {
            started,
            reply,
            waiting,
            _slot,
            ..
        } = job;
        drop(waiting);
        // A result without the acknowledgment would look like a start failure.
        observe_reply(counters, started.send(()));
        observe_reply(counters, reply.send(Err(error.clone())));
    }
}
fn observe_reply<T>(counters: &Counters, result: std::result::Result<(), T>) {
    // The error owns private reply content; classify and drop without formatting it.
    if result.is_err() {
        counters.lost(Loss::QueryReply, None);
    }
}
#[cfg(test)]
mod tests;
