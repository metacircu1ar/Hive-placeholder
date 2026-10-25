use super::*;

fn head(store: &Store) -> String {
    store
        .connection
        .query_row(
            "SELECT id FROM messages WHERE recipient='a' AND state='accepted' AND mode='next-turn' ORDER BY startup DESC,redirect DESC,seq LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn redirect_priority_preserves_startup_and_transfers_only_accepted_reservations() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    let mut a = agent("a", None);
    store.save(&a).unwrap();
    store
        .enqueue_kind("seed", "human", "a", "seed", true)
        .unwrap();
    store.enqueue("ordinary", "human", "a", "ordinary").unwrap();
    store
        .enqueue_redirect("early", "human", &mut a, "early")
        .unwrap();
    assert_eq!(head(&store), "seed");
    for id in ["seed", "early"] {
        let message = store.next_message("a").unwrap().unwrap();
        assert_eq!(message.id, id);
        store
            .message_state(&message, MessageState::Completed)
            .unwrap();
    }
    let frozen = store.reserve_turn(&mut a).unwrap().unwrap().selection;
    store.save(&a).unwrap();
    a.model = "changed".into();
    a.options_revision += 1;
    store.save(&a).unwrap();
    for id in ["r1", "r2"] {
        store.enqueue_redirect(id, "human", &mut a, id).unwrap();
    }
    let TurnReservation { message, selection } = store.reserve_turn(&mut a).unwrap().unwrap();
    assert_eq!(message.id, "r1");
    assert_eq!(selection, frozen);
    assert_eq!(head(&store), "r1");
    store
        .message_state(&message, MessageState::Submitting)
        .unwrap();
    store.enqueue_redirect("r3", "human", &mut a, "r3").unwrap();
    assert_eq!(a.reserved_turn.as_ref().unwrap().message_id, "r1");
    assert_eq!(head(&store), "r2");
    store
        .message_state(&message, MessageState::Completed)
        .unwrap();
    for id in ["r2", "r3", "ordinary"] {
        let TurnReservation {
            message,
            selection: selected,
        } = store.reserve_turn(&mut a).unwrap().unwrap();
        assert_eq!(message.id, id);
        assert_eq!(selected, a.desired_selection());
        store
            .message_state(&message, MessageState::Completed)
            .unwrap();
    }
    assert!(store.next_message("a").unwrap().is_none());
    let plan: Vec<String> = store
        .connection
        .prepare(&format!("EXPLAIN QUERY PLAN {}", queries::NEXT_MESSAGE))
        .unwrap()
        .query_map(["a"], |row| row.get(3))
        .unwrap()
        .map(std::result::Result::unwrap)
        .collect();
    assert!(
        plan.iter().any(|p| p.contains("messages_pending")),
        "{plan:?}"
    );
    assert!(!plan.iter().any(|p| p.contains("TEMP B-TREE")), "{plan:?}");
}

#[test]
fn redirect_changes_next_input_without_resetting_admission_age() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    let mut a = agent("a", None);
    store.save(&a).unwrap();
    store.save(&agent("b", None)).unwrap();
    store.enqueue("old", "human", "a", "old").unwrap();
    let oldest = store.next_message("a").unwrap().unwrap();
    store.enqueue("younger", "human", "b", "younger").unwrap();
    store
        .enqueue_redirect("redirect", "human", &mut a, "new guidance")
        .unwrap();
    let admitted = || {
        store
            .connection
            .query_row(queries::NEXT_READY_AGENT, [0], |r| r.get::<_, String>(0))
            .unwrap()
    };
    assert_eq!(admitted(), "a");
    assert_eq!(store.next_message("a").unwrap().unwrap().id, "redirect");
    store
        .message_state(&oldest, MessageState::Completed)
        .unwrap();
    assert_eq!(admitted(), "b");
    let plan: Vec<String> = store.connection.prepare(
        "EXPLAIN QUERY PLAN SELECT min(seq) FROM messages WHERE recipient=?1 AND state='accepted' AND mode='next-turn'")
        .unwrap().query_map(["a"], |r| r.get(3)).unwrap().map(std::result::Result::unwrap).collect();
    assert!(
        plan.iter()
            .any(|p| p.contains("messages_inbox")
                && p.contains("recipient=? AND state=? AND mode=?")),
        "{plan:?}"
    );
}

#[test]
fn redirect_target_is_fenced_once_and_lost_on_recovery_without_replaying_input() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    let mut a = agent("a", None);
    a.execution_id = Some("exec".into());
    store.save(&a).unwrap();
    store.enqueue("input", "human", "a", "input").unwrap();
    let input = store.reserve_turn(&mut a).unwrap().unwrap().message;
    store.save(&a).unwrap();
    store
        .message_state(&input, MessageState::Submitting)
        .unwrap();
    let first = store
        .enqueue_redirect("r1", "human", &mut a, "first")
        .unwrap()
        .unwrap();
    assert_eq!(first.message_id, "input");
    assert!(first.turn_id.is_none());
    store
        .enqueue_redirect("r2", "human", &mut a, "second")
        .unwrap();
    assert!(
        !store
            .claim_redirect("a", "other-exec", "input", "turn")
            .unwrap()
    );
    assert!(
        !store
            .claim_redirect("a", "exec", "other-input", "turn")
            .unwrap()
    );
    assert!(store.claim_redirect("a", "exec", "input", "turn").unwrap());
    assert!(!store.claim_redirect("a", "exec", "input", "turn").unwrap());
    store
        .connection
        .execute_batch(queries::RECOVER_AGENTS)
        .unwrap();
    let status: String = store
        .connection
        .query_row("SELECT state FROM redirect_interrupts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(status, "target_lost");
    let state: String = store
        .connection
        .query_row("SELECT state FROM messages WHERE id='input'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(state, "uncertain");
    assert!(!store.agent("a").unwrap().redirect_pending);
    assert_eq!(store.next_message("a").unwrap().unwrap().id, "r1");
}
