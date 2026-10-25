use super::*;

struct InputCost {
    base: FakeEmbedder,
    max_documents: Arc<AtomicUsize>,
    independent: bool,
    upstream: Arc<Mutex<()>>,
}

impl Embedder for InputCost {
    fn space(&self) -> &Space {
        self.base.space()
    }
    fn query_engine(&self) -> Option<Box<dyn Embedder>> {
        self.independent.then(|| {
            Box::new(Self {
                base: fake(Duration::ZERO),
                max_documents: self.max_documents.clone(),
                independent: false,
                upstream: self.upstream.clone(),
            }) as Box<dyn Embedder>
        })
    }
    fn embed(&mut self, inputs: &[String]) -> Result<Vec<Result<Vec<f32>>>> {
        // Distinct clients still queue behind work at one upstream resource.
        let _upstream = self.upstream.lock().unwrap();
        let document = inputs.first().is_some_and(|s| s.starts_with("d:"));
        if document {
            self.max_documents.fetch_max(inputs.len(), Ordering::SeqCst);
        }
        std::thread::sleep(Duration::from_millis(
            inputs.len() as u64 * if document { 40 } else { 2 },
        ));
        self.base.embed(inputs)
    }
}

#[test]
fn adaptive_local_backfill_keeps_short_queries_responsive_with_input_linear_cost() {
    adaptive_backfill(false);
}

#[test]
fn adaptive_http_backfill_bounds_work_queued_to_a_shared_server() {
    adaptive_backfill(true);
}

fn adaptive_backfill(independent: bool) {
    let max_documents = Arc::new(AtomicUsize::new(0));
    let engine = InputCost {
        base: fake(Duration::ZERO),
        max_documents: max_documents.clone(),
        independent,
        upstream: Arc::new(Mutex::new(())),
    };
    let (_dir, server, human) = setup(Some(Box::new(engine)));
    let state = server.state.clone();
    runtime().block_on(async {
        let running = Running::start(server);
        let text = needle_corpus(64);
        let started = std::time::Instant::now();
        call(&human, &put("concepts/corpus", &text, "create"), None)
            .await
            .unwrap();
        let published_at = started.elapsed();
        eprintln!("published at {published_at:?}; waiting for first embedding");
        until(|| coverage(&state).embedded > 0).await;
        let first_embedding_at = started.elapsed();
        let searches = simultaneous_searches(&human);
        for search in searches {
            let response = search.await.unwrap().unwrap();
            assert_eq!(response["fallback"], Value::Null);
            assert_eq!(response["mode"], "hybrid");
        }
        let queries_done_at = started.elapsed();
        eprintln!("queries finished at {queries_done_at:?}; waiting for backfill completion");
        until(|| coverage(&state).pending == 0).await;
        assert!(
            started.elapsed() < Duration::from_millis(2 * (64 * 40 + 64 * 2)),
            "Backfill exceeded twice its serial work budget: publish={published_at:?}, \
             first embedding={first_embedding_at:?}, queries={queries_done_at:?}, \
             total={:?}, independent={independent}",
            started.elapsed()
        );
        assert!((2..=5).contains(&max_documents.load(Ordering::SeqCst)));
        running.finish().await;
    });
}
