use super::*;
use crate::{ErrorCode, locks::Mutex as CheckedMutex, search::Filter};
use std::panic::{AssertUnwindSafe, catch_unwind};

fn writer_panic_after_file_commit(removal: bool) {
    let (dir, server, human) = setup(None);
    let state = server.state.clone();
    seed(&state);
    let operation = {
        let mut comb = state.writer.lock().unwrap();
        let operation = if removal {
            Operation::Remove {
                slug: "concepts/page".parse().unwrap(),
                expected_hash: comb.get(&"concepts/page".parse().unwrap()).unwrap().hash,
                request_id: "panicked".parse().unwrap(),
            }
        } else {
            put(
                "concepts/page",
                "Replacement after durable write",
                "panicked",
            )
        };
        let mut operation = operation;
        if let Operation::Put { expected_hash, .. } = &mut operation {
            *expected_hash = Some(comb.get(&"concepts/page".parse().unwrap()).unwrap().hash);
        }
        comb.before_index = Some(Box::new(|| panic!("panic after durable file commit")));
        operation
    };
    runtime().block_on(async {
        let running = Running::start(server);
        let error = call(&human, &operation, None).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::RecoveryRequired);
        assert!(error.file_committed);
        assert_eq!(error.request_id.as_deref(), Some("panicked"));
        assert_eq!(state.writes.available_permits(), 8);
        assert!(dir.path().join(".comb/intent.json").exists());
        assert_eq!(dir.path().join("concepts/page.md").exists(), !removal);
        assert_eq!(
            state.snapshot().err().unwrap().code,
            ErrorCode::RecoveryRequired
        );
        running.finish().await;
    });
    drop(state);
    let mut reopened = Comb::open(dir.path()).unwrap();
    assert!(!dir.path().join(".comb/intent.json").exists());
    assert_eq!(
        offline(&mut reopened, &operation, None).unwrap()["replayed"],
        true
    );
    assert_eq!(
        reopened.index().unwrap().stats().unwrap().pages,
        u64::from(!removal)
    );
}

#[test]
fn writer_panic_after_put_reports_possible_commit_and_recovers() {
    writer_panic_after_file_commit(false);
}

#[test]
fn writer_panic_after_unlink_reports_possible_commit_and_recovers() {
    writer_panic_after_file_commit(true);
}

fn space() -> Space {
    Space {
        backend: "fixture".into(),
        model: "poison-regression".into(),
        revision: "fixed".into(),
        dimensions: 2,
        document_prefix: "d:".into(),
        query_prefix: "q:".into(),
        preprocessing: "test".into(),
        normalization: "l2-f32".into(),
    }
}

struct Fixed {
    space: Space,
    independent: bool,
}

impl Embedder for Fixed {
    fn space(&self) -> &Space {
        &self.space
    }

    fn embed(&mut self, inputs: &[String]) -> Result<Vec<Result<Vec<f32>>>> {
        Ok(inputs.iter().map(|_| Ok(vec![1.0, 0.0])).collect())
    }

    fn query_engine(&self) -> Option<Box<dyn Embedder>> {
        self.independent.then(|| {
            Box::new(Self {
                space: self.space.clone(),
                independent: false,
            }) as Box<dyn Embedder>
        })
    }
}

fn poison_mutex<T>(mutex: &CheckedMutex<T>) {
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            let _guard = mutex.lock().unwrap();
            panic!("poison a service consistency boundary");
        }))
        .is_err()
    );
}

fn seed(state: &State) {
    state
        .writer
        .lock()
        .unwrap()
        .put_markdown(
            &"concepts/page".parse().unwrap(),
            "---\ntitle: Page\n---\nReadable summary",
            None,
            &Write::new(&Actor::Human, &"seed".parse().unwrap(), None).unwrap(),
        )
        .unwrap();
}

fn request(human: &Connection, operation: Operation) -> Request {
    Request {
        protocol: comb_protocol::compatibility::SERVICE_PROTOCOL,
        credential: human.credential.clone(),
        claim: None,
        operation,
    }
}

async fn assert_fenced(state: &Arc<State>, human: &Connection) {
    assert_eq!(
        state.snapshot().err().unwrap().code,
        ErrorCode::RecoveryRequired
    );
    assert_eq!(
        state.query("summary").await.err().unwrap().code,
        ErrorCode::RecoveryRequired
    );
    for operation in [
        Operation::Capabilities,
        Operation::Get {
            slug: "concepts/page".parse().unwrap(),
        },
        Operation::Search {
            request: Search {
                query: "summary".into(),
                ..Default::default()
            },
            mode: Mode::Text,
        },
        put("concepts/after", "Must not be written", "after"),
    ] {
        let error = state.handle(request(human, operation)).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::RecoveryRequired);
        assert!(!error.file_committed);
    }
    assert_eq!(state.reads.available_permits(), 8);
    assert_eq!(state.writes.available_permits(), 8);
}

#[derive(Clone, Copy)]
enum Domain {
    Writer,
    Engine,
    QueryEngine,
}

fn service_lock_fences_only_its_comb(domain: Domain) {
    let engine = match domain {
        Domain::Writer => None,
        Domain::Engine | Domain::QueryEngine => Some(Box::new(Fixed {
            space: space(),
            independent: matches!(domain, Domain::QueryEngine),
        }) as Box<dyn Embedder>),
    };
    let (dir, server, human) = setup(engine);
    let state = server.state.clone();
    seed(&state);
    let old = state.snapshot().unwrap();
    let (_other_dir, other, other_human) = setup(None);
    match domain {
        Domain::Writer => poison_mutex(&state.writer),
        Domain::Engine => poison_mutex(&state.model.as_ref().unwrap().engine),
        Domain::QueryEngine => {
            poison_mutex(state.model.as_ref().unwrap().query_engine.as_ref().unwrap());
        }
    }
    // No second checked acquisition has discovered the std mutex's poison yet.
    assert_eq!(
        state.health.check().unwrap_err().code,
        ErrorCode::RecoveryRequired
    );
    assert_eq!(
        old.index
            .document(&"concepts/page".parse().unwrap())
            .unwrap()
            .page
            .compiled,
        "Readable summary"
    );
    runtime().block_on(async {
        assert_fenced(&state, &human).await;
        assert_independent_comb(&other.state, &other_human).await;
    });
    assert!(!dir.path().join("concepts/after.md").exists());
    drop(old);
    drop(state);
    drop(server);
    let reopened = Comb::open(dir.path()).unwrap();
    reopened.ready().unwrap();
    reopened.get(&"concepts/page".parse().unwrap()).unwrap();
}

#[test]
fn writer_poison_fences_human_admission_but_preserves_pinned_text() {
    service_lock_fences_only_its_comb(Domain::Writer);
}

#[test]
fn local_engine_poison_fences_human_admission_but_preserves_pinned_text() {
    service_lock_fences_only_its_comb(Domain::Engine);
}

#[test]
fn independent_query_engine_poison_fences_human_admission_but_preserves_pinned_text() {
    service_lock_fences_only_its_comb(Domain::QueryEngine);
}

#[test]
fn live_registry_poison_fences_human_and_client_admission() {
    let (_dir, server, human) = setup(None);
    let state = server.state.clone();
    seed(&state);
    runtime().block_on(async {
        let running = Running::start(server);
        let client = issue(
            &human,
            Grant::Client {
                name: "Poison regression client".into(),
                reviewer: false,
            },
        )
        .await;
        call(
            &client,
            &Operation::Get {
                slug: "concepts/page".parse().unwrap(),
            },
            None,
        )
        .await
        .unwrap();
        state.registry.poison_records_for_test();
        assert_eq!(
            state.health.check().unwrap_err().code,
            ErrorCode::RecoveryRequired
        );
        for connection in [&human, &client] {
            assert_eq!(
                state
                    .registry
                    .authorize(&connection.credential, None)
                    .err()
                    .unwrap()
                    .code,
                ErrorCode::RecoveryRequired
            );
            assert_fenced(&state, connection).await;
            assert_eq!(
                call(connection, &Operation::Capabilities, None)
                    .await
                    .unwrap_err()
                    .code,
                ErrorCode::RecoveryRequired
            );
        }
        running.finish().await;
    });
}

#[test]
fn offline_capabilities_and_operations_reject_fenced_comb() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let mut comb = Comb::init(dir.path()).unwrap();
    assert_eq!(
        offline(&mut comb, &Operation::Capabilities, None).unwrap(),
        json!({"protocols":{"minimum":1,"maximum":1},"reviewer":true})
    );
    comb.health.fence();
    for operation in [
        Operation::Capabilities,
        Operation::Get {
            slug: "concepts/missing".parse().unwrap(),
        },
        put("concepts/after", "Must not be written", "after"),
    ] {
        let error = offline(&mut comb, &operation, None).unwrap_err();
        assert_eq!(error.code, ErrorCode::RecoveryRequired);
        assert!(!error.file_committed);
    }
    assert!(!dir.path().join("concepts/after.md").exists());
}

#[derive(Clone, Copy)]
enum Outcome {
    Panic,
    ProviderFailure,
}

struct Held {
    space: Space,
    calls: Arc<AtomicUsize>,
    entered: Option<tokio::sync::oneshot::Sender<()>>,
    resumed: std::sync::mpsc::Receiver<()>,
    outcome: Outcome,
}

impl Embedder for Held {
    fn space(&self) -> &Space {
        &self.space
    }

    fn embed(&mut self, _: &[String]) -> Result<Vec<Result<Vec<f32>>>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(entered) = self.entered.take() {
            entered.send(()).unwrap();
            self.resumed.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        match self.outcome {
            Outcome::Panic => panic!("document embedder panicked after entry"),
            Outcome::ProviderFailure => Err(Error::new(
                ErrorCode::FixtureUnavailable,
                "Transient fixture failure",
            )),
        }
    }
}

fn backfill_stops_after_poison(outcome: Outcome) {
    let (entered, entry) = tokio::sync::oneshot::channel();
    let (resume, resumed) = std::sync::mpsc::channel();
    let calls = Arc::new(AtomicUsize::new(0));
    let (dir, server, human) = setup(Some(Box::new(Held {
        space: space(),
        calls: calls.clone(),
        entered: Some(entered),
        resumed,
        outcome,
    })));
    let state = server.state.clone();
    seed(&state);
    runtime().block_on(async {
        let worker = tokio::spawn(state.clone().backfill());
        tokio::time::timeout(Duration::from_secs(2), entry)
            .await
            .unwrap()
            .unwrap();
        assert!(
            state.writer.is_unlocked(),
            "inference holds no writer guard"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(
            state
                .writer
                .lock()
                .unwrap()
                .embedding_worker_error
                .is_none()
        );
        if matches!(outcome, Outcome::ProviderFailure) {
            // The endpoint error reaches diagnostic recording only after the
            // writer guard has unwound and fenced this Comb.
            poison_mutex(&state.writer);
            assert_eq!(
                state.health.check().unwrap_err().code,
                ErrorCode::RecoveryRequired
            );
        }
        resume.send(()).unwrap();
        // A poisoned worker must stop, without entering the five-second retry.
        tokio::time::timeout(Duration::from_secs(2), worker)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            state.health.check().unwrap_err().code,
            ErrorCode::RecoveryRequired
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_fenced(&state, &human).await;
    });
    drop(state);
    drop(server);
    assert_unapplied_after_reopen(dir.path());
}

#[test]
fn backfill_model_panic_fences_and_stops_without_applying_results() {
    backfill_stops_after_poison(Outcome::Panic);
}

#[test]
fn backfill_provider_error_stops_when_diagnostic_writer_is_poisoned() {
    backfill_stops_after_poison(Outcome::ProviderFailure);
}

async fn assert_independent_comb(other: &Arc<State>, other_human: &Connection) {
    assert_eq!(
        other
            .handle(request(other_human, Operation::Capabilities))
            .await
            .unwrap(),
        json!({"protocols":{"minimum":1,"maximum":1},"reviewer":true})
    );
    other
        .handle(request(
            other_human,
            put("concepts/other", "Independent summary", "other"),
        ))
        .await
        .unwrap();
    let document = other
        .handle(request(
            other_human,
            Operation::Get {
                slug: "concepts/other".parse().unwrap(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(document["page"]["compiled"], "Independent summary");
}

fn assert_unapplied_after_reopen(root: &std::path::Path) {
    let reopened = Comb::open(root).unwrap();
    let coverage = reopened
        .index()
        .unwrap()
        .embedding_coverage(Filter::default())
        .unwrap();
    assert_eq!(coverage.embedded, 0);
    assert_eq!(coverage.pending, 1);
}
