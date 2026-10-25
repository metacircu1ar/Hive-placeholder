use super::*;

struct GatedFailure {
    base: FakeEmbedder,
    entered: Option<tokio::sync::oneshot::Sender<()>>,
    resume: std::sync::mpsc::Receiver<()>,
}

impl Embedder for GatedFailure {
    fn space(&self) -> &Space {
        self.base.space()
    }
    fn embed(&mut self, _: &[String]) -> Result<Vec<Result<Vec<f32>>>> {
        self.entered.take().unwrap().send(()).unwrap();
        self.resume.recv_timeout(Duration::from_secs(5)).unwrap();
        Err(Error::new(
            crate::ErrorCode::FixtureUnavailable,
            "Gated failure",
        ))
    }
}

#[test]
fn recording_embedding_failure_cannot_block_the_async_reactor() {
    let (entered, entry) = tokio::sync::oneshot::channel();
    let (resume, resumed) = std::sync::mpsc::channel();
    let engine = GatedFailure {
        base: fake(Duration::ZERO),
        entered: Some(entered),
        resume: resumed,
    };
    let (_dir, server, human) = setup(Some(Box::new(engine)));
    let state = server.state.clone();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let running = Running::start(server);
            call(&human, &put("concepts/page", "Readable", "create"), None)
                .await
                .unwrap();
            entry.await.unwrap();
            let writer = hold_writer(state.clone()).await;
            resume.send(()).unwrap();
            tokio::time::timeout(
                Duration::from_secs(2),
                call(
                    &human,
                    &Operation::Get {
                        slug: "concepts/page".parse().unwrap(),
                    },
                    None,
                ),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(
                !state.writer.is_unlocked(),
                "Failure bookkeeping must not wait on the writer in the reactor"
            );
            writer.finish().await;
            until(|| {
                state
                    .writer
                    .lock()
                    .unwrap()
                    .embedding_worker_error
                    .is_some()
            })
            .await;
            running.finish().await;
        });
}

struct WriterRelease(Option<std::sync::mpsc::Sender<()>>);
impl Drop for WriterRelease {
    fn drop(&mut self) {
        if let Some(unlock) = self.0.take() {
            let _ = unlock.send(());
        }
    }
}

struct HeldWriter {
    release: WriterRelease,
    released: tokio::sync::oneshot::Receiver<()>,
    thread: std::thread::JoinHandle<()>,
}

impl HeldWriter {
    async fn finish(self) {
        drop(self.release);
        self.released.await.unwrap();
        self.thread.join().unwrap();
    }
}

async fn hold_writer(owner: Arc<State>) -> HeldWriter {
    let (locked, lock) = tokio::sync::oneshot::channel();
    let (released, release) = tokio::sync::oneshot::channel();
    let (unlock, unlock_receiver) = std::sync::mpsc::channel();
    let release_writer = WriterRelease(Some(unlock));
    let thread = std::thread::spawn(move || {
        let guard = owner.writer.lock().unwrap();
        locked.send(()).unwrap();
        let resumed = unlock_receiver.recv_timeout(Duration::from_secs(5));
        drop(guard);
        resumed.unwrap();
        released.send(()).unwrap();
    });
    tokio::time::timeout(Duration::from_secs(2), lock)
        .await
        .unwrap()
        .unwrap();
    HeldWriter {
        release: release_writer,
        released: release,
        thread,
    }
}
