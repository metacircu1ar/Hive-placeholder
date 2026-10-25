//! Worker admission and daemon maintenance supervision.
use super::{collection, control};
use crate::error::Result;
use crate::runtime::{Shared, hub, invalid_request, provider, worker};
use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

// Recheck pending supervisor work even if no worker or capacity notification arrives.
const SUPERVISOR_RECHECK_INTERVAL: Duration = Duration::from_secs(1);

enum SupervisorEvent {
    Worker(Option<std::result::Result<String, tokio::task::JoinError>>),
    Wake,
    Signal(i32),
}
pub(in crate::runtime) struct Supervisor<'a> {
    pub(in crate::runtime) settings: &'a crate::config::Runtime,
    pub(super) shutdown: &'a AtomicBool,
    pub(super) cleanup_uncertain: &'a AtomicBool,
    pub(in crate::runtime) shared: &'a Shared,
    pub(in crate::runtime) pool: &'a provider::Pool,
    pub(in crate::runtime) wake: &'a Arc<hub::Wake>,
    pub(super) sigint: &'a mut tokio::signal::unix::Signal,
    pub(super) sigterm: &'a mut tokio::signal::unix::Signal,
    pub(super) sighup: &'a mut tokio::signal::unix::Signal,
    pub(in crate::runtime) workers: &'a mut tokio::task::JoinSet<String>,
    pub(in crate::runtime) occupied: &'a mut HashSet<String>,
    pub(in crate::runtime) starting: &'a Arc<tokio::sync::Semaphore>,
    pub(in crate::runtime) turns: &'a Arc<tokio::sync::Semaphore>,
    pub(super) maintenance: &'a mut tokio::time::Interval,
}
impl Supervisor<'_> {
    pub(super) async fn supervise(
        &mut self,
        control: &mut tokio::task::JoinHandle<Result<()>>,
    ) -> control::Supervision {
        let (shutdown, uncertain, wake) = (self.shutdown, self.cleanup_uncertain, self.wake);
        control::supervise(self.run(), control, shutdown, uncertain, wake).await
    }
    pub(super) fn stopping(&self) -> bool {
        self.shutdown.load(Ordering::Relaxed) || self.cleanup_uncertain.load(Ordering::Relaxed)
    }
    pub(super) async fn run(&mut self) -> Result<()> {
        while !self.stopping() {
            let mut remaining = collection::BATCH_LIMIT;
            self.collect_finished(&mut remaining).await?;
            if self.stopping() {
                break;
            }
            self.admit_workers().await?;
            if self.stopping() {
                break;
            }
            let pending = self.shared.with(|core| core.store().has_ready()).await?;
            // Each successful claim-release commit wakes the supervisor before
            // acknowledging this harvest, so freed slots need not await a timer.
            self.collect_finished(&mut remaining).await?;
            if self.stopping() {
                break;
            }
            if remaining == 0 {
                // Yield to signal/worker events before harvesting again.
                // An exhausted budget must not turn known completions into an eviction.
                self.wake.supervisor.notify_one();
            } else if pending && self.occupied.len() >= self.settings.max_loaded {
                self.pool.drain();
            }
            match self.next_event(pending).await {
                SupervisorEvent::Worker(done) => self.worker_finished(done).await?,
                SupervisorEvent::Wake => {}
                SupervisorEvent::Signal(signal) => {
                    let timer = crate::observability::timing::Timer::start();
                    tracing::info!(name: "daemon.signal", signal = signal, decision = "shutdown");
                    timer.finish("signal_shutdown", true);
                    break;
                }
            }
        }
        Ok(())
    }
    async fn next_event(&mut self, pending: bool) -> SupervisorEvent {
        tokio::select! {
            done = self.workers.join_next(), if !self.workers.is_empty() => {
                SupervisorEvent::Worker(done)
            }
            () = self.wake.supervisor.notified() => SupervisorEvent::Wake,
            _ = self.maintenance.tick() => SupervisorEvent::Wake,
            _ = self.sigint.recv() => SupervisorEvent::Signal(libc::SIGINT),
            _ = self.sigterm.recv() => SupervisorEvent::Signal(libc::SIGTERM),
            _ = self.sighup.recv() => SupervisorEvent::Signal(libc::SIGHUP),
            () = tokio::time::sleep(SUPERVISOR_RECHECK_INTERVAL), if pending => {
                SupervisorEvent::Wake
            },
        }
    }
}

impl Supervisor<'_> {
    pub(super) async fn admit_workers(&mut self) -> Result<()> {
        let count =
            (self.settings.max_loaded - self.occupied.len()).min(self.starting.available_permits());
        let admitted = self
            .shared
            .with(move |core| core.ready_agents(count))
            .await?;
        for id in admitted {
            let permit = self
                .starting
                .clone()
                .try_acquire_owned()
                .map_err(|cause| invalid_request().with_source(cause))?;
            self.occupied.insert(id.clone());
            let shared = self.shared.clone();
            let pool = self.pool.clone();
            let turns = self.turns.clone();
            let notification = self.wake.register(&id);
            self.workers.spawn(async move {
                let boundary = collection::WorkerBoundary::new(&id);
                worker::run(worker::WorkerInputs {
                    shared,
                    id: id.clone(),
                    pool,
                    notify: notification,
                    starting: permit,
                    turns,
                })
                .await;
                boundary.finish();
                id
            });
        }
        Ok(())
    }
}
