//! Bounded filesystem work with one explicit owner and no writer callbacks.
use super::request::{Facts, RequestInput};
use super::{Leases, WorkerInput, WorkerProof};
use crate::{
    error::{Error, Result},
    observability::{ReplyOwner, reply as observe_reply},
    runtime::hub::Wake,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot};

// Keep one filesystem worker available while capping concurrent preflight jobs at 32.
const MIN_JOBS: usize = 1;
const MAX_JOBS: usize = 32;
const _: () = assert!(MIN_JOBS <= MAX_JOBS);

enum Work {
    KeySource(
        Box<crate::auth::source::Input>,
        oneshot::Sender<Result<crate::auth::Report>>,
    ),
    #[cfg(test)]
    Test(crate::runtime::test_support::TestHooks),
    Worker(Box<WorkerInput>, oneshot::Sender<WorkerProof>),
    Request(Box<RequestInput>, oneshot::Sender<Facts>),
    Auth(
        Box<crate::runtime::preflight::auth::Input>,
        oneshot::Sender<Result<crate::runtime::preflight::auth::Prepared>>,
    ),
}
impl Work {
    fn kind(&self) -> &'static str {
        match self {
            Self::KeySource(..) => "key_source",
            #[cfg(test)]
            Self::Test(_) => "test_hold",
            Self::Worker(..) => "worker",
            Self::Request(..) => "request",
            Self::Auth(..) => "auth",
        }
    }
    fn run(self, leases: &Leases) {
        match self {
            Self::KeySource(input, reply) => {
                if !reply.is_closed() {
                    observe_reply(reply.send(input.resolve()), ReplyOwner::Preflight);
                }
            }
            Self::Auth(input, reply) => {
                if !reply.is_closed() {
                    observe_reply(reply.send(input.resolve()), ReplyOwner::Preflight);
                }
            }
            #[cfg(test)]
            Self::Test(hooks) => hooks.job(),

            Self::Worker(input, reply) => {
                if !reply.is_closed() {
                    observe_reply(reply.send(input.resolve(leases)), ReplyOwner::Preflight);
                }
            }
            Self::Request(input, reply) => {
                if !reply.is_closed() {
                    observe_reply(reply.send(input.resolve()), ReplyOwner::Preflight);
                }
            }
        }
    }
}
enum Job {
    Run(Work, OwnedSemaphorePermit),
    Stop,
}
struct Ingress {
    sender: std::sync::mpsc::Sender<Job>,
    capacity: Arc<Semaphore>,
    closed: AtomicBool,
}
#[derive(Clone)]
pub(in crate::runtime) struct Service(Arc<Ingress>);
pub(in crate::runtime) struct Owner {
    ingress: Service,
    thread: Option<std::thread::JoinHandle<()>>,
    uncertain: Arc<AtomicBool>,
}
pub(in crate::runtime) struct Started {
    pub service: Service,
    pub owner: Owner,
}
impl Service {
    pub(in crate::runtime) fn start(
        max_starting: usize,
        leases: Arc<Leases>,
        shutdown: Arc<AtomicBool>,
        uncertain: Arc<AtomicBool>,
        wake: Arc<Wake>,
    ) -> Started {
        let (sender, receiver) = std::sync::mpsc::channel();
        let service = Self(Arc::new(Ingress {
            sender,
            capacity: Arc::new(Semaphore::new(max_starting.clamp(MIN_JOBS, MAX_JOBS))),
            closed: AtomicBool::new(false),
        }));
        let failed = uncertain.clone();
        let thread = crate::runtime::owned_thread::spawn_owned_thread(
            (failed, shutdown, wake, std::cell::Cell::new("idle")),
            move |(_, _, _, phase)| {
                while let Ok(job) = receiver.recv() {
                    match job {
                        Job::Run(work, permit) => {
                            drop(permit);
                            let kind = work.kind();
                            let timer = crate::observability::timing::Timer::start();
                            phase.set(kind);
                            work.run(&leases);
                            phase.set("idle");
                            timer.finish(kind, true);
                        }
                        Job::Stop => break,
                    }
                }
            },
            |(failed, shutdown, wake, phase), result| {
                if result.is_err() {
                    tracing::error!(name: "preflight.thread_failed",
                         thread = "preflight",
                         phase = phase.get(),
                         code = "panicked",
                         decision = "shutdown");
                    failed.store(true, Ordering::Relaxed);
                    shutdown.store(true, Ordering::Relaxed);
                    wake.all();
                }
            },
        );
        Started {
            service: service.clone(),
            owner: Owner {
                ingress: service,
                thread: Some(thread),
                uncertain,
            },
        }
    }
    async fn submit(&self, work: Work) -> Result<()> {
        let permit = self
            .0
            .capacity
            .clone()
            .acquire_owned()
            .await
            .map_err(|error| stopping().with_source(error))?;
        if self.0.closed.load(Ordering::Acquire) {
            return Err(stopping());
        }
        // Preserve channel failure without retaining the unsent job's permit or leases.
        self.0.sender.send(Job::Run(work, permit)).map_err(|cause| {
            drop(cause);
            stopping().with_source(std::sync::mpsc::SendError(()))
        })
    }
    #[cfg(test)]
    pub(in crate::runtime) async fn hold(
        &self,
        entered: oneshot::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    ) -> Result<()> {
        let (reply, result) = oneshot::channel();
        let hooks = crate::runtime::test_support::TestHooks::before_job(move || {
            let _ = entered.send(());
            let _ = release.recv();
            let _ = reply.send(());
        });
        self.submit(Work::Test(hooks)).await?;
        result.await.map_err(|error| stopping().with_source(error))
    }
    async fn exchange<T>(&self, work: impl FnOnce(oneshot::Sender<T>) -> Work) -> Result<T> {
        let (reply, result) = oneshot::channel();
        self.submit(work(reply)).await?;
        result.await.map_err(|error| stopping().with_source(error))
    }
    pub(in crate::runtime) async fn worker(&self, input: WorkerInput) -> Result<WorkerProof> {
        self.exchange(|reply| Work::Worker(Box::new(input), reply))
            .await
    }
    pub(in crate::runtime) async fn key_source(
        &self,
        input: crate::auth::source::Input,
    ) -> Result<crate::auth::Report> {
        self.exchange(|reply| Work::KeySource(Box::new(input), reply))
            .await?
    }
    pub(in crate::runtime) async fn auth(
        &self,
        input: crate::runtime::preflight::auth::Input,
    ) -> Result<crate::runtime::preflight::auth::Prepared> {
        self.exchange(|reply| Work::Auth(Box::new(input), reply))
            .await?
    }
    pub(in crate::runtime) async fn request(&self, input: Box<RequestInput>) -> Result<Facts> {
        self.exchange(|reply| Work::Request(input, reply)).await
    }
}
impl Owner {
    pub(in crate::runtime) fn finish(mut self) -> Result<()> {
        self.close()
    }
    fn close(&mut self) -> Result<()> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        self.ingress.0.closed.store(true, Ordering::Release);
        self.ingress.0.capacity.close();
        observe_reply(self.ingress.0.sender.send(Job::Stop), ReplyOwner::Shutdown);
        if thread.join().is_err() {
            // The panic payload is private; the hook and owner use fixed codes.
            tracing::error!(name: "preflight.join_failed",
                 thread = "preflight",
                 code = "panicked",
                 decision = "retain_evidence");
            self.uncertain.store(true, Ordering::Relaxed);
            return Err(stopping());
        }
        Ok(())
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            tracing::warn!(name: "preflight.drop_failed",
                 thread = "preflight",
                 error = &error as &(dyn std::error::Error + 'static),
                 decision = "retain_evidence");
        }
    }
}
fn stopping() -> Error {
    Error::daemon_stopping("Filesystem preflight service is unavailable or shutting down.")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
