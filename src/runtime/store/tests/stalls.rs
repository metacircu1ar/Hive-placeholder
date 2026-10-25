use super::*;

fn quiet(store: &Store, id: &str, parent: Option<&str>) -> Agent {
    let mut a = agent(id, parent);
    a.state = AgentState::Idle;
    store.save(&a).unwrap();
    event(store, &a, EventKind::Spawned, 100);
    a
}
fn event(store: &Store, agent: &Agent, kind: EventKind, at: i64) {
    store.event(agent, kind, json!({})).unwrap();
    store
        .connection
        .execute(
            "UPDATE events SET at=? WHERE seq=(SELECT max(seq) FROM events)",
            [at],
        )
        .unwrap();
}
fn args() -> api::AgentsArgs {
    api::AgentsArgs {
        stalled_seconds: 100.into(),
        ..Default::default()
    }
}
fn page(store: &Store) -> api::StalledPage {
    store.stalled_groups(&args(), 200).unwrap()
}

#[test]
fn stalled_activity_is_latest_sequence_and_skips_infrastructure_and_active_scans() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    let mut a = quiet(&store, "a", None);
    event(&store, &a, EventKind::MessageAccepted, 180);
    event(&store, &a, EventKind::TurnFinished, 100);
    for _ in 0..200 {
        event(&store, &a, EventKind::ExecutionStopped, 199);
    }
    assert_eq!(store.idle_since("a").unwrap(), Some(100));
    assert_eq!(page(&store).groups.len(), 1);
    // Corrupt activity must not be decoded when the value is known without a walk.
    store
        .connection
        .execute("UPDATE events SET at=-1 WHERE kind='turn_finished'", [])
        .unwrap();
    assert!(store.idle_since("a").is_err());
    a.state = AgentState::Running;
    store.save(&a).unwrap();
    assert_eq!(store.idle_since("a").unwrap(), None);
    a.state = AgentState::Idle;
    a.turn_id = Some("active".into());
    store.save(&a).unwrap();
    assert_eq!(store.idle_since("a").unwrap(), None);
    a.turn_id = None;
    store.save(&a).unwrap();
    store.enqueue("m", "human", "a", "input").unwrap();
    let message = store.next_message("a").unwrap().unwrap();
    for state in [MessageState::Submitting, MessageState::Submitted] {
        store.message_state(&message, state).unwrap();
        assert_eq!(store.idle_since("a").unwrap(), None);
        assert!(page(&store).groups.is_empty());
    }
    a.task_outcome = TaskOutcome::Completed;
    a.ended_at = Some(190);
    store.save(&a).unwrap();
    assert_eq!(
        store.idle_since("a").unwrap(),
        store.agent("a").unwrap().ended_at
    );
}
#[test]
fn stalled_abnormal_end_keeps_last_known_start_without_inventing_recovery_activity() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    let mut a = quiet(&store, "a", None);
    event(&store, &a, EventKind::TurnStarted, 150);
    event(&store, &a, EventKind::ExecutionFailed, 190);
    a.state = AgentState::Failed;
    store.save(&a).unwrap();
    assert_eq!(store.idle_since("a").unwrap(), Some(150));
    assert!(page(&store).groups.is_empty());
    assert_eq!(store.stalled_groups(&args(), 250).unwrap().groups.len(), 1);
}
#[test]
fn stalled_whole_group_includes_101_independent_agents_and_unpaged_busy_child() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    quiet(&store, "root", None);
    for i in 0..100 {
        quiet(&store, &format!("child-{i:03}"), Some("root"));
    }
    let p = page(&store);
    let group = &p.groups[0];
    assert_eq!(p.groups.len(), 1);
    assert_eq!(group.root_id, "root");
    assert_eq!(group.open_count, 101);
    assert_eq!(
        group.open_preview,
        (0..20).map(|i| format!("child-{i:03}")).collect::<Vec<_>>()
    );
    assert_eq!(
        store.agent_page(0, 1, None, None).unwrap()["agents"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // Startup accepted input behind capacity is real pending work, even mail_count=0.
    store
        .enqueue_kind("startup", "root", "child-099", "work", true)
        .unwrap();
    assert!(page(&store).groups.is_empty());
}
#[test]
fn stalled_empty_paused_failed_fenced_groups_but_not_their_queued_work() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    for id in ["paused", "failed", "fenced", "ordinary"] {
        let mut a = quiet(&store, id, None);
        a.paused = id == "paused";
        if id == "failed" {
            a.state = AgentState::Failed;
        }
        if id == "fenced" {
            a.recovery_fence = Some(crate::runtime::domain::RecoveryFence::Failure(
                "requires cleanup".into(),
            ));
        }
        store.save(&a).unwrap();
    }
    let p = page(&store);
    assert_eq!(p.groups.len(), 4);
    assert!(
        p.groups
            .iter()
            .find(|g| g.root_id == "paused")
            .unwrap()
            .any_paused
    );
    for id in ["paused", "failed", "fenced", "ordinary"] {
        store.enqueue(id, "human", id, "work").unwrap();
    }
    assert!(page(&store).groups.is_empty());
}
#[test]
fn stalled_excludes_each_wait_and_approval_but_not_unused_login_holds() {
    use crate::runtime::auth_fallback::{Availability, AvailabilityState, WaitingReason};
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    let mut a = quiet(&store, "a", None);
    a.auth_group = Some("group".into());
    store.save(&a).unwrap();
    for state in [AvailabilityState::Limited, AvailabilityState::Unavailable] {
        let mut hold = Availability::available("main", a.auth_generation);
        hold.state = state;
        hold.until = Some(9000);
        store.save_availability(&hold).unwrap();
        assert_eq!(page(&store).groups.len(), 1);
    }
    for reason in [WaitingReason::Quota, WaitingReason::Switch] {
        a.waiting_reason = Some(reason);
        store.save(&a).unwrap();
        assert!(page(&store).groups.is_empty());
        a.waiting_reason = None;
        store.save(&a).unwrap();
        assert_eq!(page(&store).groups.len(), 1);
    }
    store.connection.execute("INSERT INTO approvals(id,agent,execution,turn,message,destructive,state,created_at) VALUES('q','a','e','t','question',0,'pending',100)",[]).unwrap();
    assert!(page(&store).groups.is_empty());
}
#[test]
fn stalled_missing_future_activity_threshold_and_paging() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    let a = quiet(&store, "a", None);
    quiet(&store, "b", None);
    let child = quiet(&store, "future", Some("b"));
    event(&store, &child, EventKind::TurnFinished, 201);
    store.save(&agent("missing", Some("a"))).unwrap();
    assert!(page(&store).groups.is_empty());
    event(&store, &a, EventKind::TurnFinished, 100);
    let missing = store.agent("missing").unwrap();
    event(&store, &missing, EventKind::Spawned, 100);
    assert_eq!(page(&store).groups.len(), 1);
    event(&store, &child, EventKind::TurnFinished, 100);
    let mut request = args();
    request.limit = 1.into();
    let first = store.stalled_groups(&request, 200).unwrap();
    assert_eq!(first.groups[0].root_id, "a");
    request.stalled_after = first.next_cursor.unwrap().into();
    let second = store.stalled_groups(&request, 200).unwrap();
    assert_eq!(second.groups[0].root_id, "b");
    assert!(second.next_cursor.is_none());
    request.stalled_after = second.groups[0].cursor().into();
    assert!(
        store
            .stalled_groups(&request, 200)
            .unwrap()
            .groups
            .is_empty()
    );
    assert!(
        store
            .stalled_groups(&args(), 199)
            .unwrap()
            .groups
            .is_empty()
    );
    assert!(store.stalled_groups(&args(), 99).unwrap().groups.is_empty());
}
#[test]
fn stalled_parent_boundaries_deep_tree_and_cycle_rejection() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    quiet(&store, "root", None);
    let mut middle = quiet(&store, "ended", Some("root"));
    middle.task_outcome = TaskOutcome::Completed;
    middle.ended_at = Some(150);
    store.save(&middle).unwrap();
    quiet(&store, "split", Some("ended"));
    quiet(&store, "missing", Some("absent"));
    store
        .connection
        .execute("INSERT INTO purged_agents(id) VALUES('purged')", [])
        .unwrap();
    quiet(&store, "purged-child", Some("purged"));
    assert_eq!(page(&store).groups.len(), 4);
    store.begin().unwrap();
    for i in 0..2000 {
        let p = if i == 0 {
            "root".into()
        } else {
            format!("deep-{}", i - 1)
        };
        quiet(&store, &format!("deep-{i}"), Some(&p));
    }
    store.commit().unwrap();
    assert_eq!(
        page(&store)
            .groups
            .iter()
            .find(|g| g.root_id == "root")
            .unwrap()
            .open_count,
        2001
    );
    let mut root = store.agent("root").unwrap();
    root.parent_id = Some("deep-1999".into());
    store.save(&root).unwrap();
    assert!(store.stalled_groups(&args(), 200).is_err());
}

#[test]
fn stalled_snapshot_does_not_mix_new_mail_with_old_activity() {
    let temp = crate::runtime::test_directory();
    let path = temp.path().join("db");
    let store = Store::open(&path).unwrap();
    quiet(&store, "a", None);
    let reader = Queries::open_read_only(&path).unwrap();
    reader
        .snapshot(|queries| {
            assert_eq!(queries.idle_since("a")?, Some(100));
            store.enqueue("m", "human", "a", "new input")?;
            assert_eq!(queries.stalled_groups(&args(), 200)?.groups.len(), 1);
            Ok(())
        })
        .unwrap();
    assert!(
        reader
            .stalled_groups(&args(), 200)
            .unwrap()
            .groups
            .is_empty()
    );
}

#[test]
fn stalled_response_byte_limit_produces_a_resume_cursor() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    for i in 0..4 {
        let mut a = quiet(&store, &format!("root-{i}"), None);
        a.role = Some("r".repeat(260_000));
        store.save(&a).unwrap();
    }
    let first = page(&store);
    assert!(first.groups.len() < 4);
    assert!(first.next_cursor.is_some());
    let next = api::AgentsArgs {
        stalled_after: first.next_cursor.unwrap().into(),
        ..args()
    };
    assert_eq!(
        first.groups.len() + store.stalled_groups(&next, 200).unwrap().groups.len(),
        4
    );
}
