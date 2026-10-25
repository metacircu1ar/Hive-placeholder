use super::*;

fn class(store: &Store, id: &str) -> i64 {
    store
        .connection
        .query_row(
            "SELECT admission_class FROM agents WHERE id=?1",
            [id],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn weighted_service_preserves_each_class_across_batches_and_tick_wraparound() {
    for tick in [0, usize::MAX - 3] {
        let temp = crate::runtime::test_directory();
        let mut c = core(temp.path());
        let mut owner = agent("owner", None);
        owner.paused = true;
        c.store().save(&owner).unwrap();
        // Roots arrive first, so later child seeds cannot rely on FIFO alone.
        for (prefix, parent, startup) in [
            ("3", None, true),
            ("2", Some("owner"), true),
            ("1", Some("owner"), false),
            ("0", None, false),
        ] {
            for i in 0..32 {
                let id = format!("{prefix}-{i:02}");
                c.store().save(&agent(&id, parent)).unwrap();
                c.store()
                    .enqueue_kind(&id, "human", &id, "work", startup)
                    .unwrap();
            }
        }
        c.set_admission_tick(tick);
        let mut admitted = Vec::new();
        for _ in 0..4 {
            for limit in [1, 2, 5] {
                admitted.extend(c.ready_agents(limit).unwrap());
            }
        }
        assert_eq!(admitted.iter().collect::<HashSet<_>>().len(), 32);
        for cycle in admitted.as_chunks::<8>().0 {
            for (prefix, expected) in [("0", 4), ("1", 2), ("2", 1), ("3", 1)] {
                assert_eq!(
                    cycle.iter().filter(|id| id.starts_with(prefix)).count(),
                    expected
                );
            }
        }
        for prefix in ["0", "1", "2", "3"] {
            let ids: Vec<_> = admitted
                .iter()
                .filter(|id| id.starts_with(prefix))
                .collect();
            assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
        }
        for (sql, index) in [
            (
                "EXPLAIN QUERY PLAN SELECT id FROM agents WHERE runnable=1 AND admission_class=0 ORDER BY pending_seq,id LIMIT 1",
                "agents_ready",
            ),
            (
                "EXPLAIN QUERY PLAN SELECT id FROM agents WHERE claimed=0 AND (control='stop' OR completion_pending IS NOT NULL) ORDER BY id LIMIT 128",
                "agents_controls",
            ),
        ] {
            let plan: String = c
                .store()
                .connection
                .query_row(sql, [], |r| r.get(3))
                .unwrap();
            assert!(plan.contains(index), "{plan}");
        }
    }
}

#[test]
fn unloaded_parent_and_seed_only_child_pass_older_root_seeds() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    for i in 0..100 {
        let id = format!("root-{i}");
        c.store().save(&agent(&id, None)).unwrap();
        c.store()
            .enqueue_kind(&id, "human", &id, "start", true)
            .unwrap();
    }
    let mut parent = agent("resumed", None);
    parent.state = AgentState::Stopped;
    parent.conversation_clean = true;
    parent.thread_id = Some("existing-conversation".into());
    c.store().save(&parent).unwrap();
    c.store()
        .enqueue("reply", "child", "resumed", "reply")
        .unwrap();
    c.store().save(&agent("child", Some("resumed"))).unwrap();
    c.store()
        .enqueue_kind("child-seed", "resumed", "child", "delegated task", true)
        .unwrap();
    assert_eq!(c.ready_agents(1).unwrap(), ["resumed"]);
    // The unused ordinary-mail reservation spills to child bootstrap before roots.
    assert_eq!(c.ready_agents(1).unwrap(), ["child"]);
    assert!(c.ready_agents(1).unwrap()[0].starts_with("root-"));
    assert_eq!(
        c.store().agent("resumed").unwrap().thread_id,
        parent.thread_id
    );
}

#[test]
fn head_kind_controls_admission_while_placement_keeps_coordinator_protection() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("runtime.sqlite")).unwrap();
    store.save(&agent("root", None)).unwrap();
    store.save(&agent("child", Some("root"))).unwrap();
    assert_eq!(class(&store, "root"), 3); // Initialization with no message.
    assert_eq!(class(&store, "child"), 2);
    store
        .enqueue_kind("seed", "root", "child", "start", true)
        .unwrap();
    store
        .enqueue("reply", "root", "child", "follow-up")
        .unwrap();
    assert_eq!(class(&store, "child"), 2);
    assert_eq!(store.placement("child").unwrap(), 1);
    let head = store.next_message("child").unwrap().unwrap();
    assert_eq!(head.id, "seed");
    store
        .message_state(&head, MessageState::Submitting)
        .unwrap();
    assert_eq!(class(&store, "child"), 1);
    // Nested coordinators retain the same placement even without pending mail.
    store.save(&agent("grandchild", Some("child"))).unwrap();
    assert_eq!(class(&store, "child"), 0);
    assert_eq!(store.placement("child").unwrap(), 0);
    store.defer_submission("child").unwrap();
    assert_eq!(class(&store, "child"), 2);
    assert_eq!(store.placement("child").unwrap(), 0);
    store.undeliverable("child").unwrap();
    assert_eq!(class(&store, "child"), 2);
    assert_eq!(store.placement("child").unwrap(), 0);
}
