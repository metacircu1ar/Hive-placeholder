use crate::runtime::{
    domain::{AgentState, EventKind, TaskOutcome},
    store::Store,
    test_support::AgentBuilder,
};

fn agent(store: &Store, id: &str, parent: Option<&str>) {
    let mut agent = AgentBuilder::new(id).build();
    agent.parent_id = parent.map(str::to_owned);
    agent.turn_id = None;
    agent.state = AgentState::Stopped;
    store.save(&agent).unwrap();
}

#[test]
fn subtrees_are_breadth_first_roots_are_live_and_rows_report_truncation() {
    let dir = crate::runtime::test_directory();
    let store = Store::open(&dir.path().join("db")).unwrap();
    agent(&store, "root", None);
    agent(&store, "a", Some("root"));
    agent(&store, "b", Some("root"));
    agent(&store, "a1", Some("a"));
    agent(&store, "other", None);
    assert_eq!(
        store.tree_ids(Some("root"), 10).unwrap(),
        ["root", "a", "b", "a1"]
    );
    assert_eq!(store.tree_ids(Some("a"), 10).unwrap(), ["a", "a1"]);
    // One extra row signals truncation to the caller.
    assert_eq!(store.tree_ids(Some("root"), 2).unwrap(), ["root", "a", "b"]);
    assert_eq!(store.tree_ids(Some("root"), 0).unwrap(), ["root"]);
    assert_eq!(store.tree_ids(None, 10).unwrap(), ["root", "other"]);
    let mut ended = store.agent("other").unwrap();
    ended.task_outcome = TaskOutcome::Completed;
    store.save(&ended).unwrap();
    assert_eq!(store.tree_ids(None, 10).unwrap(), ["root"]);
    assert_eq!(
        store.tree_ids(Some("missing"), 10).unwrap_err().code(),
        "unknown_agent"
    );
    let counts = store.counts("root").unwrap();
    assert_eq!((counts.children, counts.queued, counts.mail), (2, 0, 0));
    store.enqueue("m1", "human", "a", "work").unwrap();
    let row = store.tree_row(&store.agent("a").unwrap()).unwrap();
    assert_eq!((row.children, row.queued, row.mail), (1, 1, 1));
    assert_eq!(row.parent_id.as_deref(), Some("root"));
    assert!(row.approval_id.is_none());
    let mut full = store.agent("a").unwrap();
    full.sandbox = crate::config::Sandbox::FullAccess;
    store.save(&full).unwrap();
    assert_eq!(
        crate::runtime::api::value(store.tree_row(&full).unwrap())["sandbox"],
        "full-access"
    );
}

#[test]
fn tail_cursor_returns_the_newest_events() {
    let dir = crate::runtime::test_directory();
    let store = Store::open(&dir.path().join("db")).unwrap();
    agent(&store, "a", None);
    let agent = store.agent("a").unwrap();
    assert_eq!(store.tail_after("a", 2).unwrap(), 0);
    for index in 0..5 {
        store
            .event(
                &agent,
                EventKind::TurnStarted,
                serde_json::json!({"n": index}),
            )
            .unwrap();
    }
    let after = store.tail_after("a", 2).unwrap();
    let page = store.events("a", after, 2).unwrap();
    let numbers: Vec<_> = page["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["data"]["n"].clone())
        .collect();
    assert_eq!(numbers, [serde_json::json!(3), serde_json::json!(4)]);
}

#[test]
fn every_event_kind_is_observed_without_waking_and_approvals_move_the_tree() {
    let dir = crate::runtime::test_directory();
    let store = Store::open(&dir.path().join("db")).unwrap();
    agent(&store, "a", None);
    assert!(store.changes().agents.contains("a"));
    let agent = store.agent("a").unwrap();
    for value in EventKind::VALUES {
        let kind = EventKind::parse(value).unwrap();
        store.event(&agent, kind, serde_json::json!({})).unwrap();
        let changes = store.changes();
        assert!(changes.agents.is_empty() && !changes.all, "{value}");
        assert!(changes.observed.agents.contains("a"), "{value}");
        assert_eq!(
            changes.observed.tree,
            matches!(
                kind,
                EventKind::ApprovalRequested | EventKind::ApprovalClosed
            ),
            "{value}"
        );
    }
}

fn seqs(page: &serde_json::Value) -> Vec<u64> {
    page["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["seq"].as_u64().unwrap())
        .collect()
}

#[test]
fn history_pages_backwards_over_sparse_sequences_to_the_pruned_boundary() {
    let dir = crate::runtime::test_directory();
    let store = Store::open(&dir.path().join("db")).unwrap();
    agent(&store, "a", None);
    agent(&store, "b", None);
    let (a, b) = (store.agent("a").unwrap(), store.agent("b").unwrap());
    // Interleaving another agent makes a's sequences sparse.
    for index in 0..6 {
        store
            .event(&a, EventKind::TurnStarted, serde_json::json!({"n": index}))
            .unwrap();
        store
            .event(&b, EventKind::TurnStarted, serde_json::json!({}))
            .unwrap();
    }
    let all = seqs(&store.events("a", 0, 100).unwrap());
    assert_eq!(all.len(), 6);
    let newest = store.events_before("a", u64::MAX, 2).unwrap();
    assert_eq!(seqs(&newest), all[4..]);
    assert_eq!(newest["older"], all[4]);
    let middle = store.events_before("a", all[4], 2).unwrap();
    assert_eq!(seqs(&middle), all[2..4]);
    // Exactly the remaining rows: nothing older, and nothing was pruned.
    let oldest = store.events_before("a", all[2], 2).unwrap();
    assert_eq!(seqs(&oldest), all[..2]);
    assert_eq!(
        (&oldest["older"], &oldest["history_expired"]),
        (&serde_json::Value::Null, &serde_json::json!(false))
    );
    assert!(seqs(&store.events_before("a", all[0], 2).unwrap()).is_empty());
    assert_eq!(
        store.events_before("missing", 1, 2).unwrap_err().code(),
        "unknown_agent"
    );
    // Rows at or below the retention cursor are history that has expired.
    store
        .connection
        .execute(
            "INSERT INTO event_retention VALUES('a',?1)",
            [i64::try_from(all[1]).unwrap()],
        )
        .unwrap();
    let pruned = store.events_before("a", all[4], 10).unwrap();
    assert_eq!(seqs(&pruned), all[2..4]);
    assert_eq!(
        (&pruned["older"], &pruned["history_expired"]),
        (&serde_json::Value::Null, &serde_json::json!(true))
    );
    let gone = store.events_before("a", all[1], 10).unwrap();
    assert!(seqs(&gone).is_empty() && gone["history_expired"] == true);
}

#[test]
fn history_byte_budget_keeps_the_newest_rows_and_continues_older() {
    let dir = crate::runtime::test_directory();
    let store = Store::open(&dir.path().join("db")).unwrap();
    agent(&store, "a", None);
    let a = store.agent("a").unwrap();
    // Inline rows just under the artifact threshold exceed the page budget.
    let text = "x".repeat(15 * 1024);
    for index in 0..80 {
        store
            .event(
                &a,
                EventKind::TurnStarted,
                serde_json::json!({"n": index, "text": text}),
            )
            .unwrap();
    }
    // Forward pages are budgeted too, so collect every sequence across them.
    let (mut all, mut after) = (Vec::new(), 0);
    loop {
        let page = store.events("a", after, 200).unwrap();
        let next = seqs(&page);
        if next.is_empty() {
            break;
        }
        after = page["cursor"].as_u64().unwrap();
        all.extend(next);
    }
    assert_eq!(all.len(), 80);
    let page = store.events_before("a", u64::MAX, 200).unwrap();
    let kept = seqs(&page);
    assert!(kept.len() < 80 && !kept.is_empty(), "{}", kept.len());
    assert_eq!(kept, all[all.len() - kept.len()..]);
    assert_eq!(page["older"], kept[0]);
}

#[test]
fn a_spawned_child_marks_its_parent_observed_without_waking_it() {
    let dir = crate::runtime::test_directory();
    let store = Store::open(&dir.path().join("db")).unwrap();
    agent(&store, "parent", None);
    agent(&store, "child", Some("parent"));
    store.changes();
    let child = store.agent("child").unwrap();
    store
        .event(&child, EventKind::Spawned, serde_json::json!({}))
        .unwrap();
    let changes = store.changes();
    assert!(changes.agents.is_empty());
    assert!(
        changes.observed.agents.contains("parent") && changes.observed.agents.contains("child")
    );
    store
        .event(&child, EventKind::TurnStarted, serde_json::json!({}))
        .unwrap();
    assert!(!store.changes().observed.agents.contains("parent"));
}

#[test]
fn live_ids_list_every_live_agent_in_listing_order_with_truncation() {
    let dir = crate::runtime::test_directory();
    let store = Store::open(&dir.path().join("db")).unwrap();
    agent(&store, "root", None);
    agent(&store, "middle", Some("root"));
    agent(&store, "leaf", Some("middle"));
    agent(&store, "other", None);
    assert_eq!(
        store.live_ids(10).unwrap(),
        ["root", "middle", "leaf", "other"]
    );
    // An ended parent drops out while its live child stays.
    let mut ended = store.agent("middle").unwrap();
    ended.task_outcome = TaskOutcome::Completed;
    store.save(&ended).unwrap();
    assert_eq!(store.live_ids(10).unwrap(), ["root", "leaf", "other"]);
    assert_eq!(store.live_ids(1).unwrap(), ["root", "leaf"]);
}
