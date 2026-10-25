use super::*;
use crate::service::api::MutationPreparation as _;
use crate::values::ContentHash;

fn mutate(server: &Server, operation: &Operation) -> Result<Value> {
    let authority = auth::Authority::new(Actor::Human, true).unwrap();
    let api::Dispatch::Mutation(action) = operation.dispatch() else {
        panic!("fixture must describe a mutation");
    };
    let mut comb = server.state.writer.lock().unwrap();
    action.prepare(&authority)?.execute(&mut comb)
}

fn prepared() -> (tempfile::TempDir, Server, Connection, Operation) {
    let (root, server, human) = setup(None);
    let created = mutate(&server, &put("concepts/remove", "Original", "create")).unwrap();
    let operation = Operation::Remove {
        slug: "concepts/remove".parse().unwrap(),
        expected_hash: created["hash"].as_str().unwrap().parse().unwrap(),
        request_id: "remove".parse().unwrap(),
    };
    (root, server, human, operation)
}

fn heads(server: &Server) -> (Option<ContentHash>, Option<ContentHash>) {
    let comb = server.state.writer.lock().unwrap();
    let slug = "concepts/remove".parse().unwrap();
    (
        comb.disk.head(&slug).unwrap(),
        comb.index.content_hash(&slug).unwrap(),
    )
}

#[test]
fn removal_replay_and_conflicts_precede_recreated_page_reconciliation() {
    let (root, server, _human, operation) = prepared();
    assert_eq!(
        mutate(&server, &operation).unwrap(),
        json!({"removed":"concepts/remove","request_id":"remove","replayed":false})
    );
    let path = root.path().join("concepts/remove.md");
    std::fs::write(&path, "unparseable replacement").unwrap();
    assert_eq!(heads(&server), (None, None));
    assert_eq!(
        mutate(&server, &operation).unwrap(),
        json!({"removed":"concepts/remove","request_id":"remove","replayed":true})
    );
    let mut changed = operation.clone();
    if let Operation::Remove { expected_hash, .. } = &mut changed {
        *expected_hash = ContentHash::digest("another expected page");
    }
    assert_eq!(
        mutate(&server, &changed).unwrap_err().code,
        crate::ErrorCode::RequestConflict
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "unparseable replacement"
    );
    assert_eq!(heads(&server), (None, None));
    assert!(!root.path().join(".comb/intent.json").exists());
}

#[test]
fn nonhuman_removal_cannot_reconcile_a_hand_edited_target() {
    let (root, server, _human, operation) = prepared();
    let original = heads(&server);
    let path = root.path().join("concepts/remove.md");
    std::fs::write(&path, "unparseable replacement").unwrap();
    for actor in [
        Actor::Client {
            id: "client".into(),
            name: "Client".into(),
        },
        Actor::Agent {
            bridge: "bridge".into(),
            id: "agent".into(),
            role: Some("reviewer".into()),
            execution: "execution".into(),
            turn: "turn".into(),
        },
        Actor::Collector {
            bridge: "bridge".into(),
        },
    ] {
        let authority = auth::Authority::new(actor, true).unwrap();
        let api::Dispatch::Mutation(action) = operation.dispatch() else {
            panic!("fixture must describe a mutation");
        };
        let error = {
            let mut comb = server.state.writer.lock().unwrap();
            action
                .prepare(&authority)
                .and_then(|prepared| prepared.execute(&mut comb))
                .unwrap_err()
        };
        assert_eq!(error.code, crate::ErrorCode::Forbidden);
        assert!(!error.file_committed);
        assert_eq!(heads(&server), original);
    }
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "unparseable replacement"
    );
    assert!(!root.path().join(".comb/intent.json").exists());
}

#[test]
fn early_removal_replay_rejects_a_tentative_receipt_until_recovery() {
    let (root, server, _human, operation) = prepared();
    let durability = Arc::new(crate::disk::Recording::default());
    server
        .state
        .writer
        .lock()
        .unwrap()
        .disk
        .set_durability(durability.clone());
    durability.fail_finish_once();
    assert!(mutate(&server, &operation).unwrap_err().file_committed);
    {
        let comb = server.state.writer.lock().unwrap();
        assert!(
            comb.disk
                .receipt(
                    &Actor::Human.principal().unwrap(),
                    &"remove".parse().unwrap(),
                )
                .unwrap()
                .is_some()
        );
    }
    assert_eq!(
        mutate(&server, &operation).unwrap_err().code,
        crate::ErrorCode::RecoveryRequired
    );
    assert!(!root.path().join("concepts/remove.md").exists());
    assert!(root.path().join(".comb/intent.json").exists());
    assert_eq!(
        server.state.snapshot().err().unwrap().code,
        crate::ErrorCode::RecoveryRequired
    );
}

#[test]
fn lost_removal_reply_keeps_the_request_id_for_a_matching_retry() {
    let (_root, server, human, operation) = prepared();
    let state = server.state.clone();
    let guard = state.writer.lock().unwrap();
    runtime().block_on(async {
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let running = tokio::spawn(server.run(async {
            let _ = stopped.await;
        }));
        let connection = human.clone();
        let pending = operation.clone();
        let request = tokio::spawn(async move {
            client::call(&connection, &pending, None, Duration::from_millis(50)).await
        });
        embeddings::until(|| state.writes.available_permits() == 7).await;
        let error = request.await.unwrap().unwrap_err();
        assert_eq!(error.code, crate::ErrorCode::Uncertain);
        assert!(error.file_committed);
        assert_eq!(error.request_id.as_deref(), Some("remove"));
        assert!(error.message.contains("same request ID"));
        drop(guard);
        embeddings::until(|| state.writes.available_permits() == 8).await;
        assert_eq!(
            call(&human, &operation, None).await.unwrap(),
            json!({"removed":"concepts/remove","request_id":"remove","replayed":true})
        );
        stop.send(()).unwrap();
        running.await.unwrap().unwrap();
    });
}
