use super::*;
use crate::{
    ErrorCode,
    embedding::{Embedder, Space},
    search::{Mode, Search},
    service::tests::{runtime, setup},
    service::{Operation, Request, Server},
};
use std::{
    future::Future,
    panic::{AssertUnwindSafe, catch_unwind},
    pin::Pin,
    task::{Context, Wake, Waker},
    time::Duration,
};

struct PanicEngine {
    space: Space,
    gate: Option<(oneshot::Sender<()>, std::sync::mpsc::Receiver<()>)>,
}

impl PanicEngine {
    fn new() -> Self {
        Self {
            space: Space {
                backend: "fixture".into(),
                model: "query-poison".into(),
                revision: "fixed".into(),
                dimensions: 2,
                document_prefix: "d:".into(),
                query_prefix: "q:".into(),
                preprocessing: "test".into(),
                normalization: "l2-f32".into(),
            },
            gate: None,
        }
    }
}

impl Embedder for PanicEngine {
    fn space(&self) -> &Space {
        &self.space
    }

    fn embed(&mut self, _: &[String]) -> Result<Vec<Result<Vec<f32>>>> {
        if let Some((entered, resume)) = self.gate.take() {
            entered.send(()).unwrap();
            resume.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        panic!("query inference panicked");
    }
}

struct Replies {
    started: oneshot::Receiver<()>,
    result: oneshot::Receiver<Result<embedding::Query>>,
}

fn pending(state: &State) -> (Job, Replies) {
    let model = state.model.as_ref().unwrap();
    let (started, start) = oneshot::channel();
    let (reply, result) = oneshot::channel();
    let slot = state.queries.slots.clone().try_acquire_owned().unwrap();
    let waiting = PendingQueryGuard::new(model.pending_queries.clone());
    (
        Job {
            text: "query".into(),
            deadline: tokio::time::Instant::now() + START_TIMEOUT,
            started,
            reply,
            _slot: slot,
            waiting: Some(waiting),
        },
        Replies {
            started: start,
            result,
        },
    )
}

struct FencedWake {
    health: Arc<Health>,
    observed: AtomicUsize,
}

impl Wake for FencedWake {
    fn wake(self: Arc<Self>) {
        // Runs synchronously when a sender drops, before caller scheduling.
        self.observed.store(
            if self.health.check().is_err() { 1 } else { 2 },
            Ordering::SeqCst,
        );
    }
}

fn assert_released(state: &State) {
    assert_eq!(state.queries.slots.available_permits(), CAPACITY);
    assert_eq!(
        state
            .model
            .as_ref()
            .unwrap()
            .pending_queries
            .load(Ordering::SeqCst),
        0
    );
}

#[test]
fn engine_panic_fences_before_waking_the_active_reply() {
    let (_dir, server, _) = setup(Some(Box::new(PanicEngine::new())));
    let state = &server.state;
    let (job, mut replies) = pending(state);
    let wake = Arc::new(FencedWake {
        health: state.health.clone(),
        observed: AtomicUsize::new(0),
    });
    let waker = Waker::from(wake.clone());
    assert!(
        Pin::new(&mut replies.result)
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
    );
    assert!(catch_unwind(AssertUnwindSafe(|| run(state, vec![job]))).is_err());
    assert_eq!(wake.observed.load(Ordering::SeqCst), 1);
    replies.started.try_recv().unwrap();
    assert!(matches!(
        replies.result.try_recv(),
        Err(oneshot::error::TryRecvError::Closed)
    ));
    assert_released(state);
}

#[test]
fn poisoned_engine_acknowledges_and_completes_waiting_jobs() {
    let (_dir, server, _) = setup(Some(Box::new(PanicEngine::new())));
    let state = &server.state;
    let (jobs, replies): (Vec<_>, Vec<_>) = (0..3).map(|_| pending(state)).unzip();
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            let _guard = state.model.as_ref().unwrap().engine.lock().unwrap();
            panic!("poison before queued acquisition");
        }))
        .is_err()
    );
    run(state, jobs);
    for mut reply in replies {
        reply.started.try_recv().unwrap();
        let error = reply.result.try_recv().unwrap().err().unwrap();
        assert_eq!(error.code, ErrorCode::RecoveryRequired);
    }
    assert_released(state);
}

#[test]
fn model_panic_drains_semantic_and_hybrid_queries_without_leaks() {
    let (entered, entry) = oneshot::channel();
    let (resume, resumed) = std::sync::mpsc::channel();
    let mut engine = PanicEngine::new();
    engine.gate = Some((entered, resumed));
    let (_dir, server, human) = setup(Some(Box::new(engine)));
    let Server { state, queries, .. } = server;
    runtime().block_on(async {
        let worker = tokio::spawn(serve(state.clone(), queries));
        let active_owner = state.clone();
        let active = tokio::spawn(async move { active_owner.query("active").await });
        tokio::time::timeout(Duration::from_secs(2), entry)
            .await
            .unwrap()
            .unwrap();
        let waiting = waiting_searches(&state, &human);
        tokio::time::timeout(Duration::from_secs(2), async {
            while state
                .model
                .as_ref()
                .unwrap()
                .pending_queries
                .load(Ordering::SeqCst)
                != 8
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(state.queries.slots.available_permits(), CAPACITY - 9);
        resume.send(()).unwrap();
        assert_eq!(
            active.await.unwrap().err().unwrap().code,
            ErrorCode::RecoveryRequired
        );
        for caller in waiting {
            assert_eq!(
                caller.await.unwrap().unwrap_err().code,
                ErrorCode::RecoveryRequired
            );
        }
        tokio::time::timeout(Duration::from_secs(2), worker)
            .await
            .unwrap()
            .unwrap();
        assert_released(&state);
        assert_eq!(state.reads.available_permits(), 8);
        assert_eq!(state.writes.available_permits(), 8);
        assert_eq!(
            state.query("later").await.err().unwrap().code,
            ErrorCode::RecoveryRequired
        );
    });
}

struct RecordingEngine {
    space: Space,
    inputs: Arc<std::sync::Mutex<Vec<String>>>,
}
impl Embedder for RecordingEngine {
    fn space(&self) -> &Space {
        &self.space
    }
    fn embed(&mut self, inputs: &[String]) -> Result<Vec<Result<Vec<f32>>>> {
        self.inputs.lock().unwrap().extend_from_slice(inputs);
        Ok(inputs.iter().map(|_| Ok(vec![1.0, 0.0])).collect())
    }
}
#[test]
fn partial_cancellation_releases_capacity_and_keeps_live_query_order() {
    let inputs = Arc::new(std::sync::Mutex::new(Vec::new()));
    let engine = RecordingEngine {
        space: PanicEngine::new().space,
        inputs: inputs.clone(),
    };
    let (_dir, server, _) = setup(Some(Box::new(engine)));
    let state = &server.state;
    let (mut first, mut first_reply) = pending(state);
    first.text = "first".into();
    let (
        cancelled,
        Replies {
            started: mut cancelled_start,
            result,
        },
    ) = pending(state);
    drop(result);
    let (mut expired, mut expired_reply) = pending(state);
    expired.deadline = tokio::time::Instant::now() - Duration::from_secs(1);
    let (
        unacknowledged,
        Replies {
            started,
            result: mut unacknowledged_result,
        },
    ) = pending(state);
    drop(started);
    let (mut last, mut last_reply) = pending(state);
    last.text = "last".into();
    run(state, vec![first, cancelled, expired, unacknowledged, last]);
    assert_eq!(*inputs.lock().unwrap(), ["q:first", "q:last"]);
    for reply in [&mut first_reply, &mut last_reply] {
        reply.started.try_recv().unwrap();
        assert_eq!(
            reply.result.try_recv().unwrap().unwrap().vector,
            vec![1.0, 0.0]
        );
    }
    assert!(matches!(
        cancelled_start.try_recv(),
        Err(oneshot::error::TryRecvError::Closed)
    ));
    assert!(matches!(
        expired_reply.started.try_recv(),
        Err(oneshot::error::TryRecvError::Closed)
    ));
    assert!(matches!(
        expired_reply.result.try_recv(),
        Err(oneshot::error::TryRecvError::Closed)
    ));
    assert!(matches!(
        unacknowledged_result.try_recv(),
        Err(oneshot::error::TryRecvError::Closed)
    ));
    assert_released(state);
}

fn waiting_searches(
    state: &Arc<State>,
    human: &crate::service::auth::Connection,
) -> Vec<tokio::task::JoinHandle<Result<serde_json::Value>>> {
    (0..8)
        .map(|index| {
            let owner = state.clone();
            let request = Request {
                protocol: comb_protocol::compatibility::SERVICE_PROTOCOL,
                credential: human.credential.clone(),
                claim: None,
                operation: Operation::Search {
                    request: Search {
                        query: format!("waiting {index}"),
                        ..Search::default()
                    },
                    mode: if index % 2 == 0 {
                        Mode::Vector
                    } else {
                        Mode::Hybrid
                    },
                },
            };
            tokio::spawn(async move { owner.handle(request).await })
        })
        .collect::<Vec<_>>()
}
