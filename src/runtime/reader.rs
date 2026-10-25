//! One bounded read owner, separate from the transactional writer.
use super::{Core, api, hub::Wake, locks::MutexExt, read_api, store::Queries};
use crate::error::{Error, Result};
use crate::observability::{ReplyOwner, reply as observe_reply};
use serde::Serialize;
use serde_json::Value;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{mpsc, oneshot};

const QUEUE_CAPACITY: usize = 32;

#[derive(Clone, Copy)]
struct Fields {
    allocated: i64,
    cache: i32,
    pending_wakes: usize,
    actors: usize,
    leases: usize,
    readiness: Readiness,
}
#[derive(Clone, Copy)]
pub(super) struct Readiness {
    changes: u64,
    ready: bool,
}
struct Observation {
    sequence: u64,
    captured: Instant,
    unix_ms: u64,
    fields: Fields,
}
pub(super) struct Published {
    binary_sha256: String,
    backlog: Option<api::CombBacklog>,
    config: crate::config::Config,
    provider_selection: super::ProviderSelection,
    verification: Option<read_api::VerificationContext>,
    runtime_id: String,
    observation: Mutex<Observation>,
    reader_stats: Mutex<Stats>,
}
impl Published {
    pub(super) fn new(core: &Core) -> Result<Arc<Self>> {
        let readiness = Readiness {
            changes: core.store().total_changes(),
            ready: core.store().has_ready()?,
        };
        Ok(Arc::new(Self {
            binary_sha256: core.binary_sha256().to_owned(),
            backlog: core.store().retained_comb_backlog()?,
            config: core.config().clone(),
            provider_selection: core.binaries().selection.clone(),
            verification: core.verification().cloned(),
            runtime_id: uuid::Uuid::new_v4().to_string(),
            observation: Mutex::new(Self::capture(core, 0, readiness)),
            reader_stats: Mutex::default(),
        }))
    }
    fn capture(core: &Core, sequence: u64, readiness: Readiness) -> Observation {
        let super::store::WriterCounters {
            memory,
            pending_wakes,
        } = core.store().writer_counters();
        Observation {
            sequence,
            captured: Instant::now(),
            unix_ms: u64::try_from(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .inspect_err(|error| {
                        crate::observability::observation_failed(
                            "reader_clock_before_epoch",
                            error,
                        );
                    })
                    .unwrap_or_default()
                    .as_millis(),
            )
            .unwrap_or(u64::MAX),
            fields: Fields {
                allocated: memory.allocated,
                cache: memory.cache,
                pending_wakes,
                actors: core.running().len(),
                leases: core.leases().live_count(),
                readiness,
            },
        }
    }
    // Capture before taking the publication lock; it never spans SQL or file work.
    pub(super) fn prepare(&self, store: &super::store::Store) -> Result<Readiness> {
        let previous = self.observation.lock_or_panic().fields.readiness;
        let changes = store.total_changes();
        if changes == previous.changes {
            return Ok(previous);
        }
        // The candidate stays private until COMMIT succeeds. total_changes does
        // not roll back, so a rolled-back candidate must never become the cache key.
        Ok(Readiness {
            changes,
            ready: store.has_ready()?,
        })
    }
    pub(super) fn publish(&self, core: &Core, sequence: u64, readiness: Readiness) {
        let observation = Self::capture(core, sequence, readiness);
        *self.observation.lock_or_panic() = observation;
    }
    fn ping(&self) -> api::PingResponse {
        let (fields, sequence, captured, unix_ms) = {
            let observation = self.observation.lock_or_panic();
            (
                observation.fields,
                observation.sequence,
                observation.captured,
                observation.unix_ms,
            )
        };
        let mut ping = read_api::ping(
            self.binary_sha256.clone(),
            self.provider_selection.clone(),
            api::StorageDiagnostics {
                sqlite_allocated_bytes: fields.allocated,
                sqlite_cache_bytes: fields.cache,
                pending_wake_ids: fields.pending_wakes,
                ready: fields.readiness.ready,
            },
            fields.actors,
            fields.leases,
            api::WriterObservation::Committed {
                runtime_id: self.runtime_id.clone(),
                sequence,
                captured_unix_ms: unix_ms,
                age_ns: nanos(captured.elapsed()),
            },
        );
        ping.configuration.clone_from(&self.config.loaded);
        ping.deprecations.clone_from(&self.config.deprecations);
        ping.comb_backlog = self.backlog;
        ping
    }
}

pub(super) struct Reply {
    pub value: Value,
}
enum Job {
    #[cfg(feature = "comb")]
    Collectors(oneshot::Sender<Result<Vec<super::collectors::Destination>>>),
    Control(Box<ControlJob>),
    #[cfg(feature = "comb")]
    Comb(
        super::hub::Caller,
        oneshot::Sender<Result<super::comb::Attestation>>,
    ),
}
struct ControlJob {
    #[cfg(test)]
    test_hooks: super::test_support::TestHooks,
    args: api::RequestEnvelope,
    owner: Option<super::hub::Caller>,
    reply: oneshot::Sender<Result<Reply>>,
}
#[derive(Clone)]
pub(super) struct Reader {
    sender: mpsc::Sender<Job>,
    shutdown: Arc<AtomicBool>,
    published: Arc<Published>,
}
impl Reader {
    /// The daemon's configuration, as published at start.
    pub(super) fn config(&self) -> &crate::config::Config {
        &self.published.config
    }
}
pub(super) struct WeakReader {
    sender: mpsc::WeakSender<Job>,
    shutdown: Arc<AtomicBool>,
    published: Arc<Published>,
}
impl WeakReader {
    #[cfg(feature = "comb")]
    pub(super) async fn collectors(&self) -> Result<Vec<super::collectors::Destination>> {
        if self.shutdown.load(Ordering::Relaxed) {
            return Err(stopping());
        }
        let (reply, response) = oneshot::channel();
        self.sender
            .upgrade()
            .ok_or_else(stopping)?
            .send(Job::Collectors(reply))
            .await
            .map_err(|cause| stopping().with_send_source(cause))?;
        response
            .await
            .map_err(|cause| stopping().with_source(cause))?
    }

    #[cfg(feature = "comb")]
    pub(super) async fn comb(
        &self,
        caller: super::hub::Caller,
    ) -> Result<super::comb::Attestation> {
        if self.shutdown.load(Ordering::Relaxed) {
            return Err(stopping());
        }
        let (reply, response) = oneshot::channel();
        self.sender
            .upgrade()
            .ok_or_else(stopping)?
            .send(Job::Comb(caller, reply))
            .await
            .map_err(|cause| stopping().with_send_source(cause))?;
        response
            .await
            .map_err(|cause| stopping().with_source(cause))?
    }
    pub(super) async fn stalled(
        &self,
        owner: super::hub::Caller,
        args: api::RequestEnvelope,
    ) -> Result<Value> {
        if !matches!(&owner, super::hub::Caller::Agent { .. })
            || !matches!(&args.request, api::Request::Agents(args) if args.stalled_requested())
        {
            return Err(Error::unsupported_operation(
                "The agent reader accepts only stalled group queries.",
            ));
        }
        let reader = Reader {
            sender: self.sender.upgrade().ok_or_else(stopping)?,
            shutdown: self.shutdown.clone(),
            published: self.published.clone(),
        };
        reader
            .submit(args, Some(owner))
            .await
            .map(|reply| reply.value)
    }
}
pub(super) struct Started {
    pub reader: Reader,
    pub thread: std::thread::JoinHandle<Result<()>>,
}
#[derive(Clone, Copy, Default, Serialize)]
struct Stats {
    requests: u64,
    snapshots: u64,
    max_snapshot_ns: u64,
}
#[derive(Serialize)]
struct ReaderPing {
    #[serde(flatten)]
    ping: api::PingResponse,
    reader_observation: ReaderObservation,
}
#[derive(Serialize)]
struct ReaderObservation {
    source: &'static str,
    counts: Stats,
}
impl Reader {
    pub(super) fn downgrade(&self) -> WeakReader {
        WeakReader {
            sender: self.sender.downgrade(),
            shutdown: self.shutdown.clone(),
            published: self.published.clone(),
        }
    }
    /// The single FIFO owner finishes earlier artifact reads before this returns.
    /// Submit only after the corresponding deletion transaction has committed.
    pub(super) async fn barrier(&self) -> Result<()> {
        self.submit(
            api::RequestEnvelope::new(api::value(api::Request::Ping)),
            None,
        )
        .await
        .map(|_| ())
    }
    pub(super) fn start(
        queries: Queries,
        published: Arc<Published>,
        shutdown: Arc<AtomicBool>,
        uncertain: Arc<AtomicBool>,
        wake: Arc<Wake>,
    ) -> Started {
        let (sender, receiver) = mpsc::channel(QUEUE_CAPACITY);
        let flag = shutdown.clone();
        let observed = published.clone();
        let thread = super::owned_thread::spawn_owned_thread(
            (observed, flag, uncertain, wake),
            move |(observed, flag, _, wake)| serve(queries, receiver, observed, flag, wake),
            |(_, flag, uncertain, wake), result| {
                if !matches!(result, Ok(Ok(()))) {
                    let error = match result {
                        Ok(Err(error)) => error.clone(),
                        _ => Error::daemon_stopping("Control reader did not finish normally."),
                    };
                    wake.failed(&error);
                    flag.store(true, Ordering::Relaxed);
                    if result.is_err() {
                        uncertain.store(true, Ordering::Relaxed);
                    }
                    wake.all();
                }
            },
        );
        Started {
            reader: Self {
                sender,
                shutdown,
                published,
            },
            thread,
        }
    }
    pub(super) async fn request(
        &self,
        caller: &super::hub::Caller,
        args: api::RequestEnvelope,
    ) -> Result<Reply> {
        if !caller.is_human() || !args.request.is_read_only() {
            return Err(Error::unsupported_operation(
                "The control reader accepts only human read operations.",
            ));
        }
        if matches!(args.request, api::Request::Ping) {
            if self.shutdown.load(Ordering::Relaxed) || self.sender.is_closed() {
                return Err(stopping());
            }
            let ping = self.published.ping();
            let counts = *self.published.reader_stats.lock_or_panic();
            let value = api::value(ReaderPing {
                ping,
                reader_observation: ReaderObservation {
                    source: "completed_reader_jobs",
                    counts,
                },
            });
            return Ok(Reply { value });
        }
        self.submit(args, None).await
    }
    async fn submit(
        &self,
        args: api::RequestEnvelope,
        owner: Option<super::hub::Caller>,
    ) -> Result<Reply> {
        if self.shutdown.load(Ordering::Relaxed) {
            return Err(stopping());
        }
        let (reply, response) = oneshot::channel();
        self.sender
            .send(Job::Control(Box::new(ControlJob {
                #[cfg(test)]
                test_hooks: super::test_support::TestHooks::default(),
                args,
                owner,
                reply,
            })))
            .await
            .map_err(|cause| stopping().with_send_source(cause))?;
        response
            .await
            .map_err(|cause| stopping().with_source(cause))?
    }
}
fn serve(
    queries: Queries,
    mut receiver: mpsc::Receiver<Job>,
    published: &Published,
    shutdown: &AtomicBool,
    wake: &Wake,
) -> Result<()> {
    let mut stats = Stats::default();
    while let Some(job) = receiver.blocking_recv() {
        let job = match job {
            Job::Control(job) => *job,
            #[cfg(feature = "comb")]
            Job::Collectors(reply) => {
                if !reply.is_closed() {
                    let result = if shutdown.load(Ordering::Relaxed) {
                        Err(stopping())
                    } else {
                        queries.collector_destinations()
                    };
                    observe_reply(reply.send(result), ReplyOwner::Reader);
                }
                continue;
            }

            #[cfg(feature = "comb")]
            Job::Comb(caller, reply) => {
                answer_comb(&queries, &caller, reply, shutdown, wake)?;
                continue;
            }
        };
        #[cfg(test)]
        job.test_hooks.job();
        if job.reply.is_closed() {
            continue;
        }
        if shutdown.load(Ordering::Relaxed) {
            observe_reply(job.reply.send(Err(stopping())), ReplyOwner::Reader);
            continue;
        }
        let read = |queries: &Queries| {
            if let Some(owner) = &job.owner {
                owner.check_read(queries)?;
            }
            read_api::dispatch(
                queries,
                &job.args.request,
                published.verification.as_ref(),
                &published.config,
            )
        };
        let result = if matches!(job.args.request, api::Request::Ping) {
            Ok(api::value(published.ping()))
        } else if read_api::artifact(&job.args.request) {
            read(&queries)
        } else {
            let start = Instant::now();
            let result = queries.snapshot(read);
            stats.snapshots += 1;
            stats.max_snapshot_ns = stats.max_snapshot_ns.max(nanos(start.elapsed()));
            result
        };
        stats.requests += 1;
        *published.reader_stats.lock_or_panic() = stats;
        if let Some(error) = unhealthy_result(&queries, &result) {
            wake.failed(&error);
            observe_reply(job.reply.send(Err(error.clone())), ReplyOwner::Reader);
            return Err(error);
        }
        let result = result.map(|value| Reply { value });
        observe_reply(job.reply.send(result), ReplyOwner::Reader);
    }
    Ok(())
}
#[cfg(feature = "comb")]
fn answer_comb(
    queries: &Queries,
    caller: &super::hub::Caller,
    reply: oneshot::Sender<Result<super::comb::Attestation>>,
    shutdown: &AtomicBool,
    wake: &Wake,
) -> Result<()> {
    if reply.is_closed() {
        return Ok(());
    }
    let result = if shutdown.load(Ordering::Relaxed) {
        Err(stopping())
    } else {
        queries.snapshot(|queries| comb_identity(queries, caller))
    };
    let failure = unhealthy_result(queries, &result);
    if let Some(error) = &failure {
        wake.failed(error);
    }
    observe_reply(reply.send(result), ReplyOwner::Reader);
    failure.map_or(Ok(()), Err)
}

#[cfg(feature = "comb")]
fn comb_identity(
    queries: &Queries,
    caller: &super::hub::Caller,
) -> Result<super::comb::Attestation> {
    let agent = caller.checked_agent(queries)?;
    if agent.terminal() || agent.control.is_some() || agent.turn_id.is_none() {
        return Err(Error::stale_execution(
            "Comb tool belongs to an ended or interrupted turn.",
        ));
    }
    let binding = agent
        .comb
        .ok_or_else(|| Error::unsupported_operation("No Comb is configured for this agent."))?;
    let super::hub::Caller::Agent {
        execution,
        turn: Some(turn),
        ..
    } = caller
    else {
        return Err(Error::stale_execution(
            "Comb requires an active agent turn.",
        ));
    };
    Ok(super::comb::Attestation {
        tool_catalog: agent.tool_catalog,
        binding,
        claim: comb_protocol::service::auth::Claim::Agent {
            id: agent.id,
            role: agent.role,
            execution: execution.clone(),
            turn: turn.clone(),
        },
    })
}
fn unhealthy_result<T>(queries: &Queries, result: &Result<T>) -> Option<Error> {
    let health = queries.healthy().and_then(|()| {
        if queries.autocommit() {
            Ok(())
        } else {
            Err(Error::database_error(
                "The control reader did not release its snapshot.",
            ))
        }
    });
    health
        .err()
        .map(|error| result.as_ref().err().cloned().unwrap_or(error))
}
fn stopping() -> Error {
    Error::daemon_stopping("Control reader is unavailable or shutting down.")
}
fn nanos(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Fixtures and assertions fail immediately.
mod tests;
