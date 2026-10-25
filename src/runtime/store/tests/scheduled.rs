use super::super::scheduled::queries::{CANCEL, DUE, NEXT};
use super::*;
use crate::runtime::scheduled::not_before;
use std::time::Duration;

fn schedule(c: &Core, id: &str, sender: &str, at: u64) {
    c.store()
        .enqueue_scheduled(id, sender, "a", "future-secret", at)
        .unwrap();
    if sender == "human" {
        c.store().track_call(id).unwrap();
    }
}
fn scalar(c: &Core, sql: &str) -> i64 {
    c.store()
        .connection
        .query_row(sql, [], |r| r.get(0))
        .unwrap()
}
fn setup(c: &Core) {
    let mut a = agent("a", None);
    a.state = AgentState::Stopped;
    c.store().save(&a).unwrap();
    let mut sender = agent("s", None);
    sender.turn_id = Some("original-turn".into());
    c.store().save(&sender).unwrap();
    c.store().changes();
}
#[test]
fn scheduled_rounding_is_never_early_and_overflow_is_rejected() {
    assert_eq!(not_before(Duration::from_secs(99), 1).unwrap(), 100);
    assert_eq!(not_before(Duration::new(99, 900_000_000), 1).unwrap(), 101);
    assert_eq!(not_before(Duration::new(99, 1), 604_800).unwrap(), 604_900);
    assert!(not_before(Duration::from_secs(i64::MAX as u64), 1).is_err());
    assert!(not_before(Duration::from_secs(u64::MAX), 1).is_err());
}
#[test]
fn scheduled_promotion_preserves_proof_and_gets_fresh_queue_age() {
    let tmp = crate::runtime::test_directory();
    let mut c = core(tmp.path());
    setup(&c);
    schedule(&c, "delayed", "s", 100);
    schedule(&c, "human", "human", 100);
    let accepted = scalar(
        &c,
        "SELECT accepted_at FROM call_results WHERE message_id='human'",
    );
    let changes = c.store().changes();
    assert!(changes.deadlines);
    assert!(changes.agents.is_empty());
    assert!(!changes.all);
    assert_eq!(c.store().agent("a").unwrap().state, AgentState::Stopped);
    assert_eq!(c.store().next_deadline().unwrap(), Some(100));
    assert_eq!(
        scalar(&c, "SELECT pending_count FROM agents WHERE id='a'"),
        0
    );
    let pending = c.store().message_result("human").unwrap();
    assert_eq!(pending.delivery_state, MessageState::Scheduled);
    assert_eq!(pending.outcome, api::CallOutcome::Pending);
    assert!(!pending.queue_count_exact);
    assert_eq!(pending.queued_ahead, None);
    assert_eq!(pending.not_before, Some(100));
    assert_eq!(c.promote_scheduled(99).unwrap(), 0);
    c.store().enqueue("ordinary", "s", "a", "now").unwrap();
    assert_eq!(c.promote_scheduled(100).unwrap(), 2);
    assert_eq!(
        scalar(&c, "SELECT pending_count FROM agents WHERE id='a'"),
        3
    );
    assert_eq!(c.store().next_message("a").unwrap().unwrap().id, "ordinary");
    assert_eq!(scalar(&c, "SELECT seq FROM messages WHERE id='delayed'"), 4);
    assert_eq!(scalar(&c, "SELECT seq FROM messages WHERE id='human'"), 5);
    assert_eq!(scalar(&c, "SELECT pending_seq FROM agents WHERE id='a'"), 3);
    assert_eq!(
        scalar(
            &c,
            "SELECT count(*) FROM deliveries WHERE status='accepted'"
        ),
        3
    );
    assert_eq!(
        scalar(
            &c,
            "SELECT count(*) FROM deliveries WHERE message_id='delayed' AND sender_turn='original-turn'"
        ),
        1
    );
    assert_eq!(
        scalar(
            &c,
            "SELECT accepted_at FROM call_results WHERE message_id='human'"
        ),
        accepted
    );
    assert_eq!(c.store().next_deadline().unwrap(), None);
    assert_eq!(
        c.store().message_result("human").unwrap().queued_ahead,
        Some(2)
    );
    assert_eq!(
        scalar(
            &c,
            "SELECT count(*) FROM events WHERE kind='message_promoted'"
        ),
        2
    );
}
#[test]
fn scheduled_batches_are_bounded_and_sql_failure_rolls_back_moves() {
    let tmp = crate::runtime::test_directory();
    let mut c = core(tmp.path());
    setup(&c);
    for n in 0..130 {
        schedule(&c, &format!("m{n}"), "s", 100);
    }
    assert_eq!(c.promote_scheduled(100).unwrap(), 128);
    c.store().enqueue("interleaved", "s", "a", "now").unwrap();
    assert_eq!(c.promote_scheduled(100).unwrap(), 2);
    assert!(
        scalar(&c, "SELECT seq FROM messages WHERE id='m128'")
            > scalar(&c, "SELECT seq FROM messages WHERE id='interleaved'")
    );
    schedule(&c, "rollback", "human", 200);
    let old = scalar(&c, "SELECT seq FROM messages WHERE id='rollback'");
    let last = scalar(&c, "SELECT seq FROM sqlite_sequence WHERE name='messages'");
    c.store().connection.execute_batch("CREATE TEMP TRIGGER reject_move BEFORE UPDATE ON deliveries WHEN new.message_id='rollback' BEGIN SELECT RAISE(ABORT,'fault after move'); END;").unwrap();
    assert!(c.promote_scheduled(200).is_err());
    assert_eq!(
        scalar(&c, "SELECT seq FROM messages WHERE id='rollback'"),
        old
    );
    assert_eq!(
        scalar(&c, "SELECT seq FROM sqlite_sequence WHERE name='messages'"),
        last
    );
    assert_eq!(
        c.store().message_result("rollback").unwrap().delivery_state,
        MessageState::Scheduled
    );
    assert_eq!(
        scalar(&c, "SELECT pending_count FROM agents WHERE id='a'"),
        131
    );
    c.store()
        .connection
        .execute_batch("DROP TRIGGER reject_move")
        .unwrap();
    drop(c);
    let mut c = core(tmp.path());
    assert_eq!(c.promote_scheduled(200).unwrap(), 1);
    assert_eq!(c.promote_scheduled(200).unwrap(), 0);
}
#[test]
fn scheduled_corruption_is_quarantined_without_changing_delivery_evidence() {
    let tmp = crate::runtime::test_directory();
    let mut c = core(tmp.path());
    setup(&c);
    for id in ["missing", "contradictory", "missing-call", "valid"] {
        schedule(&c, id, "human", 100);
    }
    c.store().connection.execute_batch("DELETE FROM call_results WHERE message_id='missing'; DELETE FROM deliveries WHERE message_id='missing'; DELETE FROM call_results WHERE message_id='missing-call'; UPDATE deliveries SET status='submitted',execution='e',recipient_turn='t',error='original-error' WHERE message_id='contradictory'; UPDATE call_results SET output='invalid JSON but preserved',start_seq=5,output_final=1 WHERE message_id='contradictory';").unwrap();
    let seq = scalar(&c, "SELECT seq FROM messages WHERE id='contradictory'");
    assert_eq!(c.promote_scheduled(100).unwrap(), 4);
    assert_eq!(
        scalar(
            &c,
            "SELECT count(*) FROM events WHERE kind='message_schedule_failed'"
        ),
        3
    );
    assert_eq!(
        scalar(&c, "SELECT seq FROM messages WHERE id='contradictory'"),
        seq
    );
    assert_eq!(
        scalar(
            &c,
            "SELECT count(*) FROM deliveries WHERE message_id='contradictory' AND status='submitted' AND execution='e' AND recipient_turn='t' AND error='original-error'"
        ),
        1
    );
    assert_eq!(
        scalar(
            &c,
            "SELECT count(*) FROM call_results WHERE message_id='contradictory' AND output='invalid JSON but preserved' AND start_seq=5 AND output_final=1"
        ),
        1
    );
    for id in ["missing", "contradictory", "missing-call"] {
        assert_eq!(
            c.store().message_result(id).unwrap_err().code(),
            "database_error"
        );
    }
    assert_eq!(
        c.store().message_result("valid").unwrap().delivery_state,
        MessageState::Accepted
    );
    let mut a = c.store().agent("a").unwrap();
    a.execution_id = Some("e".into());
    c.store().save(&a).unwrap();
    c.store().enqueue("real", "s", "a", "real").unwrap();
    c.store().connection.execute_batch("UPDATE messages SET state='submitted' WHERE id='real'; UPDATE deliveries SET execution='e',recipient_turn='t' WHERE message_id='real'").unwrap();
    assert_eq!(
        c.store().cutoff_input(&a, "t").unwrap().as_deref(),
        Some("real")
    );
    assert!(c.store().continuation_origin("a", "contradictory").is_err());
    c.store()
        .connection
        .execute_batch("DELETE FROM events")
        .unwrap();
    drop(c);
    let mut c = core(tmp.path());
    assert_eq!(c.promote_scheduled(999).unwrap(), 0);
    assert_eq!(c.store().next_deadline().unwrap(), None);
    assert_eq!(
        c.store()
            .message_result("contradictory")
            .unwrap_err()
            .code(),
        "database_error"
    );
    assert_eq!(scalar(&c, "SELECT count(*) FROM events"), 0);
}
#[test]
fn scheduled_lifecycle_gates_stalls_and_handoff() {
    let tmp = crate::runtime::test_directory();
    let mut c = core(tmp.path());
    setup(&c);
    let mut a = c.store().agent("a").unwrap();
    a.paused = true;
    c.store().save(&a).unwrap();
    c.store().event(&a, EventKind::Spawned, json!({})).unwrap();
    c.store()
        .connection
        .execute_batch("UPDATE events SET at=1")
        .unwrap();
    let args = api::AgentsArgs {
        stalled_seconds: 1.into(),
        ..Default::default()
    };
    assert_eq!(
        c.store().stalled_groups(&args, 100).unwrap().groups.len(),
        1
    );
    schedule(&c, "m", "s", 100);
    assert!(
        c.store()
            .stalled_groups(&args, 100)
            .unwrap()
            .groups
            .is_empty()
    );
    assert!(
        !c.store()
            .auth_handoff("a", crate::runtime::auth_fallback::AuthReason::Manual)
            .unwrap()
            .contains("future-secret")
    );
    let outgoing = c
        .store()
        .auth_handoff("s", crate::runtime::auth_fallback::AuthReason::Manual)
        .unwrap();
    assert!(outgoing.contains("not_before"));
    assert!(!outgoing.contains("future-secret"));
    c.store()
        .connection
        .execute("DELETE FROM agents WHERE id='s'", [])
        .unwrap();
    assert_eq!(c.promote_scheduled(100).unwrap(), 1);
    assert!(c.store().agent("a").unwrap().paused);
    assert_eq!(c.store().agent("a").unwrap().state, AgentState::Stopped);
    assert_eq!(
        scalar(
            &c,
            "SELECT count(*) FROM deliveries WHERE sender_turn='original-turn'"
        ),
        1
    );
    schedule(&c, "ended", "human", 200);
    c.store().undeliverable("a").unwrap();
    assert_eq!(
        c.store().message_result("ended").unwrap().outcome,
        api::CallOutcome::Cancelled
    );
    assert_eq!(c.promote_scheduled(200).unwrap(), 0);
}
#[test]
fn scheduled_queries_use_indexes_and_global_ids_guard_continuations() {
    let tmp = crate::runtime::test_directory();
    let c = core(tmp.path());
    setup(&c);
    for (sql, index) in [
        (DUE.replace("?1", "100"), "messages_scheduled"),
        (NEXT.to_owned(), "messages_scheduled"),
        (CANCEL.replace("?1", "'a'"), "messages_inbox"),
        (queries::STALLED_AGENTS.to_owned(), "messages_inbox"),
    ] {
        let plan = c
            .store()
            .connection
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap()
            .query_map([], |r| r.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .join("\n");
        assert!(plan.contains(index), "{plan}");
        assert!(!plan.contains("TEMP B-TREE"), "{plan}");
    }
    assert!(
        c.store()
            .enqueue_continuation("c", "a", "continue", "root", 1)
            .unwrap()
    );
    assert!(
        !c.store()
            .enqueue_continuation("c", "a", "continue", "root", 1)
            .unwrap()
    );
    assert_eq!(
        c.store()
            .enqueue_continuation("c", "s", "continue", "root", 1)
            .unwrap_err()
            .code(),
        "database_error"
    );
    assert!(
        c.store()
            .enqueue_scheduled("c", "s", "a", "conflict", 100)
            .is_err()
    );
    assert_eq!(
        scalar(
            &c,
            "SELECT count(*) FROM deliveries WHERE message_id='c' AND status='accepted'"
        ),
        1
    );
}

#[test]
fn scheduled_promotion_keeps_gates_and_recovery_never_replays_attempted_input() {
    use crate::runtime::auth_fallback::WaitingReason;
    for gate in ["paused", "failed", "quota", "fenced"] {
        let tmp = crate::runtime::test_directory();
        let mut c = core(tmp.path());
        setup(&c);
        let mut a = c.store().agent("a").unwrap();
        match gate {
            "paused" => a.paused = true,
            "failed" => a.state = AgentState::Failed,
            "quota" => {
                a.waiting_reason = Some(WaitingReason::Quota);
                a.quota_wait_until = Some(999);
            }
            _ => {
                a.recovery_fence =
                    Some(crate::runtime::domain::RecoveryFence::ExecutionCleanupUncertain);
                c.store()
                    .connection
                    .execute("UPDATE agents SET blocked=1 WHERE id='a'", [])
                    .unwrap();
            }
        }
        c.store().save(&a).unwrap();
        schedule(&c, "m", "human", 100);
        c.promote_scheduled(100).unwrap();
        assert_eq!(
            c.store().message_result("m").unwrap().delivery_state,
            MessageState::Accepted
        );
        assert_eq!(
            scalar(&c, "SELECT runnable FROM agents WHERE id='a'"),
            0,
            "{gate}"
        );
        assert_eq!(
            c.store().agent("a").unwrap().waiting_reason,
            a.waiting_reason
        );
        assert_eq!(
            c.store().agent("a").unwrap().recovery_fence,
            a.recovery_fence
        );
        assert_eq!(c.store().next_deadline().unwrap(), None);
    }
    let tmp = crate::runtime::test_directory();
    let mut c = core(tmp.path());
    setup(&c);
    schedule(&c, "future", "human", 200);
    schedule(&c, "attempted", "human", 100);
    c.promote_scheduled(100).unwrap();
    let input = c.store().next_message("a").unwrap().unwrap();
    c.store()
        .message_state(&input, MessageState::Submitted)
        .unwrap();
    c.store().recover_agents().unwrap();
    assert_eq!(
        c.store().message_result("future").unwrap().delivery_state,
        MessageState::Scheduled
    );
    assert_eq!(
        c.store()
            .message_result("attempted")
            .unwrap()
            .delivery_state,
        MessageState::Uncertain
    );
    c.promote_scheduled(200).unwrap();
    assert_eq!(c.store().next_message("a").unwrap().unwrap().id, "future");
}

#[test]
fn scheduled_quarantine_marker_survives_actual_compaction() {
    let tmp = crate::runtime::test_directory();
    let mut c = core(tmp.path());
    let root = tmp.path().join("state");
    state::directory(&root).unwrap();
    let runtime = root.join("runtime");
    state::directory(&runtime).unwrap();
    c.replace_store(Store::open(&runtime.join("runtime.sqlite")).unwrap());
    setup(&c);
    schedule(&c, "broken", "human", 100);
    schedule(&c, "pending", "human", 999);
    c.store().connection.execute_batch("UPDATE deliveries SET status='failed',execution='old-execution' WHERE message_id='broken'; UPDATE call_results SET accepted_at=0,output='{}' WHERE message_id='broken'").unwrap();
    c.promote_scheduled(100).unwrap();
    let mut a = c.store().agent("a").unwrap();
    a.state = AgentState::Stopped;
    a.task_outcome = TaskOutcome::Completed;
    c.store().save(&a).unwrap();
    drop(c);
    let report = compact(&root, crate::state::now() + 1, 1).unwrap();
    assert_eq!(report.call_results_removed, 1);
    let s = Store::open(&runtime.join("runtime.sqlite")).unwrap();
    assert_eq!(
        s.message_result("broken").unwrap_err().code(),
        "database_error"
    );
    assert_eq!(
        s.message_result("pending").unwrap().delivery_state,
        MessageState::Scheduled
    );
    assert_eq!(
        s.messages("a", None, 200).unwrap()["messages"][1]["schedule_integrity"],
        "invalid_delivery_proof"
    );
}
