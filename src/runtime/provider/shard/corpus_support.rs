//! Owned shard driver construction and typed transport adapters for tests.
use super::init::{ShardChannels, ShardIdentity, channels};
use super::*;
use crate::runtime::provider::RpcHarness;
use crate::runtime::{provider::session::Session, test_support::RuntimeFixture};
use serde_json::json;

pub(crate) struct ShardSetup {
    pub shard: Arc<Shard>,
    pub queues: ShardChannels,
}

pub(crate) fn shard(fixture: &RuntimeFixture) -> ShardSetup {
    let (inputs, queues) = channels([256; 3]).unwrap();
    let shard = Arc::new(Shard::new(
        ShardIdentity {
            key: None,
            id: "generation".into(),
            auth: "main",
            process_options: "",
            backend: &super::super::Backend::Codex,
            compatibility: None,
            lease: &fixture.lease,
        },
        inputs,
        false,
        || 0,
    ));
    ShardSetup { shard, queues }
}

pub(super) struct RouteOutput {
    pub events: mpsc::Receiver<Frame>,
    pub failure: watch::Receiver<Option<Error>>,
}

pub(super) struct RouteSetup {
    pub route: Route,
    pub output: RouteOutput,
}
impl Route {
    pub(super) fn for_test(capacity: usize, attach_cancelled: Arc<AtomicBool>) -> RouteSetup {
        let (tx, events) = mpsc::channel(capacity);
        let (failure, failed) = watch::channel(None);
        RouteSetup {
            route: Self {
                attach_cancelled,
                tx,
                bytes: Arc::default(),
                failure,
            },
            output: RouteOutput {
                events,
                failure: failed,
            },
        }
    }
}

impl RouteOutput {
    pub(crate) fn received(&mut self) -> Option<Value> {
        self.events
            .try_recv()
            .ok()
            .map(|frame| frame.value.into_raw())
    }
}

pub(super) fn with_driver<T>(
    fixture: &RuntimeFixture,
    rpc: &mut RpcHarness,
    action: impl FnOnce(&mut ShardLoop<'_>, &mut RouteOutput) -> T,
) -> T {
    with_backend(fixture, rpc, super::super::Backend::Codex, action)
}

pub(super) fn with_backend<T>(
    fixture: &RuntimeFixture,
    rpc: &mut RpcHarness,
    backend: super::super::Backend,
    action: impl FnOnce(&mut ShardLoop<'_>, &mut RouteOutput) -> T,
) -> T {
    let pool = fixture.pool();
    let ShardSetup {
        mut shard,
        mut queues,
    } = shard(fixture);
    Arc::get_mut(&mut shard).unwrap().backend = backend;
    let started = Started {
        pid: rpc.rpc.pid(),
        before: 0,
        after: 0,
    };
    let RouteSetup { route, mut output } = Route::for_test(256, Arc::default());
    let mut driver = ShardLoop::new(&pool, &shard, &mut rpc.rpc, &started, &mut queues);
    driver.threads.insert("thread".into(), "agent".into());
    driver.routes.insert("agent".into(), route);
    action(&mut driver, &mut output)
}

pub(super) fn route(driver: &mut ShardLoop<'_>, frame: IncomingFrame) -> Result<()> {
    let bytes = frame.raw().to_string().len();
    driver.rpc.pending.push_back(frame, bytes);
    driver.drain_frames()
}

pub(super) struct RequestSetup {
    pub id: u64,
    pub reply: oneshot::Receiver<Result<Response>>,
}

pub(super) fn request(
    driver: &mut ShardLoop<'_>,
    request: outgoing::OutgoingRequest,
    tombstone: bool,
) -> RequestSetup {
    assert!(driver.pending.is_empty());
    let (reply, result) = oneshot::channel();
    driver
        .handle_command(
            Cmd::Call("agent".into(), request, reply),
            &mut Instant::now(),
        )
        .unwrap();
    assert_eq!(driver.pending.len(), 1);
    let (&id, pending) = driver.pending.iter_mut().next().unwrap();
    if tombstone {
        pending.reply.take();
        pending.deadline = None;
    }
    RequestSetup { id, reply: result }
}

pub(crate) struct WorkerTransport {
    pub session: Session,
    pub sent: Arc<Mutex<Vec<Value>>>,
    task: tokio::task::JoinHandle<()>,
}

impl WorkerTransport {
    pub(crate) fn new(fixture: &RuntimeFixture, thread_result: Value) -> Self {
        let ShardSetup { shard, mut queues } = shard(fixture);
        shard
            .catalog
            .set(Ok(vec![Model::parse(&json!({"model":"gpt-5.6-luna",
            "supportedReasoningEfforts":[{"reasoningEffort":"low"}]}))]))
            .unwrap();
        let (_tx, events) = mpsc::channel(1);
        let (_failure, failed) = watch::channel(None);
        let session = Session {
            pool: fixture.pool(),
            shard,
            agent: "agent".into(),
            events,
            failure: failed,
            started: Started {
                pid: 0,
                before: 0,
                after: 0,
            },
            thread: None,
            // This test transport owns no provider reservation to quarantine on drop.
            released: true,
        };
        let sent = Arc::new(Mutex::new(Vec::new()));
        let captured = sent.clone();
        let task = tokio::spawn(async move {
            loop {
                let command = tokio::select! {
                    command = queues.normal.recv() => command,
                    command = queues.priority.recv() => command,
                    command = queues.essential.recv() => command,
                };
                match command {
                    Some(Cmd::Send(value, reply)) => {
                        captured
                            .lock_or_panic()
                            .push(crate::runtime::api::value(value));
                        let _ = reply.send(Ok(()));
                    }
                    Some(Cmd::Call(_, request, reply)) => {
                        let method = request.method();
                        captured
                            .lock_or_panic()
                            .push(json!({"method":method,"params":request}));
                        let result = if method == ClientMethod::TurnSteer {
                            Err(Error::unsupported_operation(
                                "Corpus provider has no steering support.",
                            ))
                        } else {
                            Ok(Response::parse(thread_result.clone()))
                        };
                        let _ = reply.send(result);
                    }
                    None => break,
                    _ => panic!("unexpected corpus transport command"),
                }
                // Keep the wake reader alive for the real Session::command write.
                let mut bytes = [0; 256];
                let _ = queues.wake.read(&mut bytes);
            }
        });
        Self {
            session,
            sent,
            task,
        }
    }

    pub(crate) async fn finish(self) {
        drop(self.session);
        self.task.await.unwrap();
    }
}
