//! Own the database writer and commit each batch before its acknowledgements or wakeups.
mod attachments;
mod requests;
mod timing;
mod writer;
pub(super) use super::caller::Caller;
use super::{Core, locks::MutexExt};
use crate::error::{Error, Result};
use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
};
use tokio::sync::{Notify, mpsc, oneshot};

// Maximum database jobs queued before submitters must wait for the writer.
const JOB_QUEUE_CAPACITY: usize = 256;
// Limit jobs per commit so acknowledgements are not held by unbounded batching.
const COMMIT_JOB_LIMIT: usize = 64;
// Stop coalescing after this interval without preempting a running job.
const COMMIT_TIME_BUDGET: std::time::Duration = std::time::Duration::from_millis(4);
// Minimum interval between checkpoints, which run only when the job queue is empty.
const CHECKPOINT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

type Completion = Box<dyn FnOnce(Result<()>) + Send>;
struct Job {
    apply: Box<dyn FnOnce(&mut Core) -> Completion + Send>,
    queued: crate::observability::timing::Timer,
}
impl Job {
    fn new<T: Send + 'static, G: Send + 'static>(
        f: impl FnOnce(&mut Core) -> Result<T> + Send + 'static,
        evidence: G,
    ) -> (Self, oneshot::Receiver<Result<T>>) {
        let (tx, rx) = oneshot::channel();
        let job = Self {
            queued: crate::observability::timing::Timer::start(),
            apply: Box::new(move |core| {
                let result = core.atomic(f);
                Box::new(move |commit| {
                    if let (Err(primary), Err(secondary)) = (&result, &commit) {
                        tracing::error!(name: "writer.job_and_commit_failed",
                             code = primary.code(),
                             primary = primary as &(dyn std::error::Error + 'static),
                             secondary = secondary as &(dyn std::error::Error + 'static),
                             sqlite_code = super::store::sqlite_code(secondary),
                             decision = "preserve_job_error");
                    }
                    crate::observability::reply(
                        tx.send(result.and_then(|value| commit.map(|()| value))),
                        crate::observability::ReplyOwner::Writer,
                    );
                    drop(evidence);
                })
            }),
        };
        (job, rx)
    }
    fn run(self, core: &mut Core, timing: &mut timing::Batch) -> Completion {
        timing.received(self.queued);
        (self.apply)(core)
    }
}

#[derive(Default)]
pub struct Wake {
    pub supervisor: Notify,
    pub deadlines: Notify,
    /// Never-persisted live items and viewer signals; see `runtime::live`.
    pub live: super::live::LiveHub,
    agents: Mutex<HashMap<String, Arc<Notify>>>,
    failure: std::sync::OnceLock<Error>,
}
impl Wake {
    /// Record an owner failure before waking dependents or closing their channels.
    pub(super) fn failed(&self, error: &Error) {
        if let Err(secondary) = self.failure.set(error.clone()) {
            tracing::debug!(name: "runtime.secondary_failure",
                 code = secondary.code(),
                 error = &secondary as &(dyn std::error::Error + 'static),
                 decision = "preserve_primary");
        }
    }
    pub(super) fn failure(&self) -> Option<Error> {
        self.failure.get().cloned()
    }
    pub fn register(&self, id: &str) -> Arc<Notify> {
        self.agents
            .lock_or_panic()
            .entry(id.into())
            .or_default()
            .clone()
    }
    pub fn remove(&self, id: &str) {
        self.agents.lock_or_panic().remove(id);
    }
    pub fn all(&self) {
        for n in self.agents.lock_or_panic().values() {
            n.notify_one();
        }
        self.supervisor.notify_one();
        self.deadlines.notify_one();
    }
    fn agent(&self, id: &str) {
        if let Some(n) = self.agents.lock_or_panic().get(id) {
            n.notify_one();
        }
    }
}

pub(super) struct StartedHub {
    pub hub: Hub,
    pub writer: std::thread::JoinHandle<Result<()>>,
    pub preflight: super::preflight::Owner,
}

#[derive(Clone)]
pub struct Hub {
    pub(super) attachment_gate: Arc<tokio::sync::Mutex<()>>,
    #[cfg(feature = "comb")]
    pub(super) comb_slots: Arc<tokio::sync::Semaphore>,
    sender: mpsc::Sender<Job>,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
    uncertain: Arc<std::sync::atomic::AtomicBool>,
    pub(super) reader: Arc<std::sync::OnceLock<super::reader::WeakReader>>,
    pub(super) preflight: super::preflight::Service,
    pub wake: Arc<Wake>,
    pub(super) published: Arc<super::reader::Published>,
    pub(super) catalogs: Arc<super::provider::catalogs::Catalogs>,
}
impl Hub {
    pub fn start(
        core: Core,
        wake: Arc<Wake>,
        uncertain: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<StartedHub> {
        // Fail before starting services if the initial committed state cannot be read.
        let published = super::reader::Published::new(&core)?;
        let super::preflight::Started {
            service: preflight,
            owner,
        } = super::preflight::Service::start(
            core.config().runtime.max_starting,
            core.leases().clone(),
            core.shutdown().clone(),
            uncertain.clone(),
            wake.clone(),
        );
        let catalogs = Arc::new(super::provider::catalogs::Catalogs::new(
            core.config().runtime.clone(),
            core.shutdown().clone(),
            uncertain.clone(),
            wake.clone(),
        ));
        let observation = published.clone();
        let (sender, receiver) = mpsc::channel::<Job>(JOB_QUEUE_CAPACITY);
        let notify = wake.clone();
        let shutdown = core.shutdown().clone();
        let context = (
            shutdown.clone(),
            uncertain.clone(),
            wake.clone(),
            core.daemon_lock().clone(),
        );
        let thread = super::owned_thread::spawn_owned_thread(
            context,
            move |_| writer::WriterLoop::new(core, receiver, observation, notify).run(),
            |(shutdown, uncertain, wake, _owner), result| {
                if result.is_err() {
                    wake.failed(&Error::daemon_stopping(
                        "Runtime writer did not finish normally.",
                    ));
                    uncertain.store(true, std::sync::atomic::Ordering::Relaxed);
                    shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
                    wake.all();
                }
            },
        );
        Ok(StartedHub {
            hub: Self {
                attachment_gate: Arc::default(),
                #[cfg(feature = "comb")]
                comb_slots: Arc::new(tokio::sync::Semaphore::new(64)),
                sender,
                shutdown,
                uncertain,
                reader: Arc::default(),
                preflight,
                wake,
                published,
                catalogs,
            },
            writer: thread,
            preflight: owner,
        })
    }
    /// A failed lifecycle/fence write cannot enqueue its own shutdown on the writer.
    pub(super) fn stop_after_failure(&self, error: &Error) {
        self.wake.failed(error);
        self.uncertain
            .store(true, std::sync::atomic::Ordering::Release);
        self.shutdown
            .store(true, std::sync::atomic::Ordering::Release);
        self.wake.all();
    }
    pub fn with<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Core) -> Result<T> + Send + 'static,
    ) -> impl Future<Output = Result<T>> + Send {
        self.submit(f, ())
    }
    /// The writer owns evidence through COMMIT even if the requesting task is cancelled.
    pub(super) fn with_retained<T: Send + 'static, G: Send + 'static>(
        &self,
        evidence: G,
        f: impl FnOnce(&mut Core) -> Result<T> + Send + 'static,
    ) -> impl Future<Output = Result<T>> + Send {
        self.submit(f, evidence)
    }
    async fn submit<T: Send + 'static, G: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Core) -> Result<T> + Send + 'static,
        evidence: G,
    ) -> Result<T> {
        let (job, rx) = Job::new(f, evidence);
        self.sender
            .send(job)
            .await
            .map_err(|cause| closed().with_send_source(cause))?;
        rx.await.map_err(|cause| closed().with_source(cause))?
    }
    pub fn blocking_with<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Core) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let (job, rx) = Job::new(f, ());
        self.sender
            .blocking_send(job)
            .map_err(|cause| closed().with_send_source(cause))?;
        rx.blocking_recv()
            .map_err(|cause| closed().with_source(cause))?
    }
}
fn closed() -> Error {
    Error::daemon_stopping("Runtime store is unavailable.")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Fixtures fail immediately.
mod failure_tests;
