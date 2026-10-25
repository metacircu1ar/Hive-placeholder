//! Own daemon service startup, supervision, and the ordered shutdown of all producers.
use super::{
    Arc, AtomicBool, Core, DaemonSignals, Error, MAINTENANCE_INTERVAL, Ordering, Queries, Result,
    Supervisor, UnixListener, control, daemon_signals, finish_store, hub, join_daemon_workers,
    provider, reader,
};
use std::collections::HashSet;

pub(super) struct ServiceInput<'a> {
    pub core: Core,
    pub queries: Queries,
    pub settings: &'a crate::config::Runtime,
    pub shutdown: &'a Arc<AtomicBool>,
    pub cleanup_uncertain: &'a Arc<AtomicBool>,
    pub recovered: Vec<provider::RecoveredReservation>,
}
// Field drop order mirrors the original reverse local order on startup failure.
pub(super) struct Services {
    auth_task: tokio::task::JoinHandle<Result<()>>,
    #[cfg(feature = "comb")]
    collector_task: tokio::task::JoinHandle<Result<()>>,
    pool: provider::Pool,
    reader_thread: std::thread::JoinHandle<Result<()>>,
    reader: reader::Reader,
    preflight: super::super::preflight::Owner,
    writer: std::thread::JoinHandle<Result<()>>,
    shared: hub::Hub,
    wake: Arc<hub::Wake>,
}
impl ServiceInput<'_> {
    pub fn start(self) -> Result<Services> {
        let Self {
            core,
            queries,
            settings,
            shutdown,
            cleanup_uncertain,
            recovered,
        } = self;
        let wake = Arc::new(hub::Wake::default());
        let hub::StartedHub {
            hub: shared,
            writer,
            preflight,
        } = hub::Hub::start(core, wake.clone(), cleanup_uncertain.clone())?;
        let reader::Started {
            reader,
            thread: reader_thread,
        } = reader::Reader::start(
            queries,
            shared.published.clone(),
            shutdown.clone(),
            cleanup_uncertain.clone(),
            wake.clone(),
        );
        assert!(
            shared.reader.set(reader.downgrade()).is_ok(),
            "reader installed once"
        );
        let pool = provider::Pool::new(
            settings.clone(),
            shared.clone(),
            cleanup_uncertain.clone(),
            shutdown.clone(),
            recovered,
        );
        let auth_task = start_auth_owner(shared.clone(), pool.clone(), shutdown.clone());
        #[cfg(feature = "comb")]
        let collector_task = {
            let collector_hub = shared.clone();
            let collector_shutdown = shutdown.clone();
            tokio::spawn(async move {
                let result = super::super::collectors::run(
                    collector_hub.clone(),
                    collector_shutdown.clone(),
                )
                .await;
                if let Err(error) = &result {
                    collector_hub.wake.failed(error);
                    collector_shutdown.store(true, Ordering::Release);
                    collector_hub.wake.all();
                }
                result
            })
        };
        Ok(Services {
            auth_task,
            #[cfg(feature = "comb")]
            collector_task,
            pool,
            reader_thread,
            reader,
            preflight,
            writer,
            shared,
            wake,
        })
    }
}

fn start_auth_owner(
    shared: hub::Hub,
    pool: provider::Pool,
    shutdown: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<Result<()>> {
    tokio::spawn(async move {
        let result = super::super::auth_fallback::run(shared.clone(), pool, shutdown.clone()).await;
        if let Err(error) = &result {
            shared.wake.failed(error);
            shutdown.store(true, Ordering::Release);
            shared.wake.all();
        }
        result
    })
}

fn join_reader(thread: std::thread::JoinHandle<Result<()>>, uncertain: &AtomicBool) -> Result<()> {
    thread.join().unwrap_or_else(|_| {
        tracing::error!(name: "reader.join_failed",
             thread = "reader",
             code = "panicked",
             decision = "retain_evidence");
        uncertain.store(true, Ordering::Relaxed);
        Err(Error::daemon_stopping(
            "Control reader did not finish normally.",
        ))
    })
}

struct SupervisorResources {
    maintenance: tokio::time::Interval,
    turns: Arc<tokio::sync::Semaphore>,
    starting: Arc<tokio::sync::Semaphore>,
    occupied: HashSet<String>,
    workers: tokio::task::JoinSet<String>,
    signals: DaemonSignals,
}
pub(super) struct DaemonRun {
    control_task: tokio::task::JoinHandle<Result<()>>,
    control_stop: tokio::sync::oneshot::Sender<()>,
    resources: SupervisorResources,
}
pub(super) struct FinishedDaemon {
    pub result: Result<()>,
    // Signal registrations and admission owners survive final diagnostics and owner cleanup.
    _resources: SupervisorResources,
    _wake: Arc<hub::Wake>,
}
impl DaemonRun {
    pub fn start(
        listener: UnixListener,
        settings: &crate::config::Runtime,
        services: &Services,
        shutdown: &Arc<AtomicBool>,
    ) -> Result<Self> {
        let Services {
            shared,
            reader,
            pool,
            ..
        } = services;
        let listener =
            tokio::net::UnixListener::from_std(listener).map_err(Error::state_io_from)?;
        let signals = daemon_signals()?;
        let workers = tokio::task::JoinSet::new();
        let occupied = HashSet::<String>::new();
        let starting = Arc::new(tokio::sync::Semaphore::new(settings.max_starting));
        let turns = pool.turns.clone();
        let mut maintenance = tokio::time::interval(MAINTENANCE_INTERVAL);
        maintenance.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let control::Started {
            stop: control_stop,
            task: control_task,
        } = control::start(
            listener,
            shared.clone(),
            reader.clone(),
            pool.clone(),
            shutdown.clone(),
        );
        Ok(Self {
            resources: SupervisorResources {
                maintenance,
                turns,
                starting,
                occupied,
                workers,
                signals,
            },
            control_stop,
            control_task,
        })
    }

    pub async fn supervise(
        &mut self,
        services: &Services,
        settings: &crate::config::Runtime,
        shutdown: &AtomicBool,
        cleanup_uncertain: &AtomicBool,
    ) -> control::Supervision {
        let Services {
            shared, pool, wake, ..
        } = services;
        let SupervisorResources {
            signals,
            workers,
            occupied,
            starting,
            turns,
            maintenance,
        } = &mut self.resources;
        Supervisor {
            settings,
            shutdown,
            cleanup_uncertain,
            shared,
            pool,
            wake,
            sigint: &mut signals.sigint,
            sigterm: &mut signals.sigterm,
            sighup: &mut signals.sighup,
            workers,
            occupied,
            starting,
            turns,
            maintenance,
        }
        .supervise(&mut self.control_task)
        .await
    }
    pub async fn finish(
        self,
        services: Services,
        supervision: control::Supervision,
        shutdown: &AtomicBool,
        cleanup_uncertain: &AtomicBool,
    ) -> FinishedDaemon {
        let Services {
            #[cfg(feature = "comb")]
            collector_task,
            auth_task,
            pool,
            reader_thread,
            reader,
            preflight,
            writer,
            shared,
            wake,
        } = services;
        let Self {
            mut resources,
            control_stop,
            control_task,
        } = self;
        let control::Supervision {
            supervisor: loop_result,
            control: control_result,
        } = supervision;
        shutdown.store(true, Ordering::Relaxed);
        wake.all();
        crate::observability::reply(
            control_stop.send(()),
            crate::observability::ReplyOwner::Shutdown,
        );
        join_daemon_workers(&mut resources.workers, cleanup_uncertain).await;
        let control_result = control::join(control_task, control_result, cleanup_uncertain).await;
        #[cfg(not(feature = "comb"))]
        let collector_result = Ok(());
        #[cfg(feature = "comb")]
        let collector_result = collector_task
            .await
            .map_err(|cause| Error::internal_error("Collector owner failed.").with_source(cause))
            .and_then(|result| result);
        drop(reader);
        let reader_result = join_reader(reader_thread, cleanup_uncertain);
        let auth_result = auth_task
            .await
            .map_err(|cause| Error::internal_error("Deadline owner failed.").with_source(cause))
            .and_then(|result| result);
        shared.catalogs.stop().await;
        pool.shutdown().await;
        drop(pool);
        let preflight_result = preflight.finish();
        drop(shared);
        let writer_result = finish_store(writer, cleanup_uncertain);
        let result = super::logging::results([
            ("supervisor", loop_result),
            ("control", control_result),
            ("reader", reader_result),
            ("preflight", preflight_result),
            ("auth", auth_result),
            ("collector", collector_result),
            ("writer", writer_result),
        ]);
        FinishedDaemon {
            result: wake.failure().map_or(result, Err),
            _resources: resources,
            _wake: wake,
        }
    }
}
