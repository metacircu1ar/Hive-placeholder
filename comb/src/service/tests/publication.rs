use super::*;

async fn seed_writer(human: &Connection) -> Value {
    let created = call(
        human,
        &put("components/writer", "Old summary", "create"),
        None,
    )
    .await
    .unwrap();
    call(
        human,
        &put("components/unrelated", "Untouched summary", "other"),
        None,
    )
    .await
    .unwrap();
    created
}

fn original_instance(state: &State) -> String {
    state
        .snapshot()
        .unwrap()
        .index
        .test_connection()
        .query_row(
            "SELECT instance FROM chunks WHERE slug='components/unrelated'",
            [],
            |r| r.get(0),
        )
        .unwrap()
}

async fn converge_hand_edit(root: &std::path::Path, state: &State, human: &Connection) {
    let created = seed_writer(human).await;
    let old_instance = original_instance(state);
    let mut raw = std::fs::read_to_string(root.join("components/writer.md")).unwrap();
    raw = raw.replace("Old summary", "Hand edited summary");
    std::fs::write(root.join("components/writer.md"), raw).unwrap();
    let mut edit = put("components/writer", "Agent update", "update");
    if let Operation::Put { expected_hash, .. } = &mut edit {
        *expected_hash = Some(created["hash"].as_str().unwrap().parse().unwrap());
    }
    assert_eq!(
        call(human, &edit, None).await.unwrap_err().code,
        crate::ErrorCode::Conflict
    );
    let current = get(human, "components/writer").await;
    assert_eq!(current["unverified"], true);
    assert_eq!(current["page"]["metadata"]["external"], true);
    if let Operation::Put { expected_hash, .. } = &mut edit {
        *expected_hash = Some(current["hash"].as_str().unwrap().parse().unwrap());
    }
    call(human, &edit, None).await.unwrap();
    assert_eq!(
        state
            .snapshot()
            .unwrap()
            .index
            .test_connection()
            .query_row(
                "SELECT instance FROM chunks WHERE slug='components/unrelated'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        old_instance
    );
}

#[test]
fn hand_edit_cas_converges_without_replacing_unrelated_chunk_identity() {
    let (dir, server, human) = setup(None);
    let state = server.state.clone();
    runtime().block_on(async {
        let running = Running::start(server);
        converge_hand_edit(dir.path(), &state, &human).await;
        running.finish().await;
    });
}

#[test]
fn reads_progress_during_file_flush_after_hand_edit_converges() {
    let (dir, server, human) = setup(None);
    let state = server.state.clone();
    runtime().block_on(async {
        let running = Running::start(server);
        converge_hand_edit(dir.path(), &state, &human).await;
        let (entered, entry) = tokio::sync::oneshot::channel();
        let (resume, resumed) = std::sync::mpsc::channel();
        state.writer.lock().unwrap().before_index = Some(Box::new(move || {
            entered.send(()).unwrap();
            resumed.recv_timeout(Duration::from_secs(10)).unwrap();
        }));
        let writer = human.clone();
        let write = tokio::spawn(async move {
            call(&writer, &put("concepts/new", "A new page", "paused"), None).await
        });
        entry.await.unwrap();
        // Markdown has been replaced, but the writer has not published the index.
        let doc = tokio::time::timeout(
            Duration::from_secs(2),
            call(
                &human,
                &Operation::Get {
                    slug: "components/writer".parse().unwrap(),
                },
                None,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(doc["page"]["compiled"], "Agent update");
        let found = call(
            &human,
            &Operation::Search {
                request: Search {
                    query: "summary".into(),
                    ..Default::default()
                },
                mode: Mode::Text,
            },
            None,
        )
        .await
        .unwrap();
        assert!(!found["results"].as_array().unwrap().is_empty());
        resume.send(()).unwrap();
        write.await.unwrap().unwrap();
        running.finish().await;
    });
}

#[test]
fn finalization_failure_blocks_new_reads_but_preserves_pinned_snapshot() {
    let (dir, server, _) = setup(None);
    let state = server.state.clone();
    let write = Write::new(&Actor::Human, &"initial".parse().unwrap(), None).unwrap();
    state
        .writer
        .lock()
        .unwrap()
        .put_markdown(
            &"concepts/page".parse().unwrap(),
            "---\ntitle: Page\n---\nOld",
            None,
            &write,
        )
        .unwrap();
    let old = state.snapshot().unwrap();
    let durability = Arc::new(crate::disk::Recording::default());
    state
        .writer
        .lock()
        .unwrap()
        .disk
        .set_durability(durability.clone());
    durability.fail_finish_once();
    let error = state
        .writer
        .lock()
        .unwrap()
        .append(
            &"concepts/page".parse().unwrap(),
            "New evidence",
            "test",
            None,
            None,
            &Write::new(&Actor::Human, &"failed-finalize".parse().unwrap(), None).unwrap(),
        )
        .unwrap_err();
    assert!(error.file_committed);
    assert_eq!(
        state.snapshot().err().unwrap().code,
        crate::ErrorCode::RecoveryRequired
    );
    assert!(
        old.index
            .document(&"concepts/page".parse().unwrap())
            .unwrap()
            .page
            .timeline
            .is_empty()
    );
    drop(old);
    drop(state);
    drop(server);
    assert_reopened_evidence(dir.path());
}

fn assert_reopened_evidence(root: &std::path::Path) {
    let comb = Comb::open(root).unwrap();
    assert_eq!(
        comb.get(&"concepts/page".parse().unwrap())
            .unwrap()
            .page
            .timeline
            .len(),
        1
    );
}
