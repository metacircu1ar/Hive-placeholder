use super::*;
use crate::runtime::api::joins::{JoinOutcome, JoinState, MEMBER_BYTES};

fn setup(store: &Store, requester: &str, count: usize, deadline: Option<u64>) -> String {
    store
        .atomic(|s| {
            if requester != "human" {
                s.save(&agent(requester, None))?;
            }
            let join = s.create_join(requester, "round", deadline)?;
            for position in 0..count {
                let target = format!("worker-{position}");
                s.save(&agent(&target, (requester != "human").then_some(requester)))?;
                let message = format!("input-{position}");
                s.enqueue_kind(&message, requester, &target, "Return the answer.", true)?;
                s.join_member(&join, requester, &message, position)?;
            }
            s.join_requested(&join, requester, "round", count)?;
            Ok(join)
        })
        .unwrap()
}
fn consume(store: &Store, position: usize) -> (Agent, Message) {
    let mut a = store.agent(&format!("worker-{position}")).unwrap();
    let message = store.next_message(&a.id).unwrap().unwrap();
    a.execution_id = Some(format!("execution-{position}"));
    a.thread_id = Some(format!("thread-{position}"));
    a.turn_id = Some(format!("turn-{position}"));
    store.save(&a).unwrap();
    store
        .message_state(&message, MessageState::Submitted)
        .unwrap();
    store
        .turn_started(
            &a,
            api::events::TurnStarted {
                attachments: &[],
                turn_id: a.turn_id.as_deref().unwrap(),
                message_id: &message.id,
                sender: &message.sender,
                selection: api::events::TurnSelection {
                    model: &a.model,
                    effort: &a.effort,
                    options_revision: a.options_revision,
                    selection_verification: "requested_unverified",
                },
            },
        )
        .unwrap();
    (a, message)
}
fn say(store: &Store, a: &Agent, message: &Message, text: &str, phase: &str) {
    store
        .assistant_output(
            a,
            Some(&message.id),
            json!({"turn_id":a.turn_id,"text":text,"phase":phase}),
            true,
        )
        .unwrap();
}
fn finish(store: &Store, position: usize, outcome: MessageState, text: &str) {
    store
        .atomic(|s| {
            let (a, message) = consume(s, position);
            say(s, &a, &message, text, "final_answer");
            s.message_state(&message, outcome)
        })
        .unwrap();
}
fn join_artifact_count(store: &Store, join: &str) -> i64 {
    store
        .query_row_cached(
            "SELECT count(*) FROM artifacts WHERE join_id=?1",
            [join],
            |r| r.get(0),
        )
        .unwrap()
}
fn results(store: &Store, requester: &str) -> Vec<String> {
    store
        .sql()
        .query_column(
            "SELECT body FROM messages WHERE recipient=?1 AND kind='join_result' ORDER BY seq",
            [requester],
        )
        .unwrap()
}
#[test]
fn join_collects_exact_terminal_outcomes_and_delivers_once() {
    let temp = crate::runtime::test_directory();
    let s = Store::open(&temp.path().join("db")).unwrap();
    let join = setup(&s, "requester", 6, None);
    for (position, outcome) in [
        MessageState::Completed,
        MessageState::Failed,
        MessageState::Interrupted,
        MessageState::Uncertain,
        MessageState::Undeliverable,
    ]
    .into_iter()
    .enumerate()
    {
        finish(&s, position, outcome, &format!("answer-{position}"));
        assert!(results(&s, "requester").is_empty());
    }
    s.atomic(|s| s.undeliverable("worker-5")).unwrap();
    let snapshot = s.join_snapshot(&join).unwrap();
    assert_eq!(snapshot.state, JoinState::Completed);
    assert_eq!(
        snapshot
            .members
            .iter()
            .map(|m| m.outcome)
            .collect::<Vec<_>>(),
        vec![
            JoinOutcome::Completed,
            JoinOutcome::Failed,
            JoinOutcome::Interrupted,
            JoinOutcome::Uncertain,
            JoinOutcome::Undeliverable,
            JoinOutcome::Undeliverable
        ]
    );
    assert!(!snapshot.members[0].output_partial);
    assert!(snapshot.members[1].output_partial);
    let output = results(&s, "requester");
    assert_eq!(output.len(), 1);
    assert!(output[0].contains("answer-0"));
    s.atomic(|s| {
        s.settle_joins()?;
        s.expire_join_deadlines(u64::MAX / 2)?;
        Ok(())
    })
    .unwrap();
    assert_eq!(results(&s, "requester"), output);
    assert!(s.has_ready().unwrap());
}
#[test]
fn join_deadline_restart_final_contains_only_remaining_members() {
    let temp = crate::runtime::test_directory();
    let path = temp.path().join("db");
    let s = Store::open(&path).unwrap();
    let at = crate::state::now() + 1;
    let join = setup(&s, "requester", 3, Some(at));
    finish(&s, 0, MessageState::Completed, "early-unique");
    s.atomic(|s| s.expire_join_deadlines(at)).unwrap();
    assert_eq!(
        s.join_snapshot(&join).unwrap().state,
        JoinState::DeadlinePassed
    );
    let partial = results(&s, "requester");
    assert_eq!(partial.len(), 1);
    assert!(partial[0].contains("pending"));
    assert!(partial[0].contains("early-unique"));
    drop(s);
    let s = Store::open(&path).unwrap();
    finish(&s, 1, MessageState::Completed, "later-one");
    s.atomic(|s| s.expire_join_deadlines(at + 100)).unwrap();
    assert_eq!(results(&s, "requester").len(), 1);
    finish(&s, 2, MessageState::Failed, "later-two");
    let outputs = results(&s, "requester");
    assert_eq!(outputs.len(), 2);
    assert!(!outputs[1].contains("early-unique"));
    assert!(outputs[1].contains("later-one"));
    assert!(outputs[1].contains("later-two"));
    assert!(
        s.join_snapshot(&join)
            .unwrap()
            .members
            .iter()
            .all(|m| m.delivered)
    );
}
#[test]
fn join_human_deadline_observation_and_completion_race() {
    for deadline_first in [false, true] {
        let temp = crate::runtime::test_directory();
        let s = Store::open(&temp.path().join("db")).unwrap();
        let at = crate::state::now() + 1;
        let join = setup(&s, "human", 1, Some(at));
        if deadline_first {
            s.atomic(|s| s.expire_join_deadlines(at)).unwrap();
            assert_eq!(
                s.join_snapshot(&join).unwrap().state,
                JoinState::DeadlinePassed
            );
        }
        s.atomic(|s| {
            let (a, m) = consume(s, 0);
            say(s, &a, &m, "human answer", "final_answer");
            s.message_state(&m, MessageState::Completed)?;
            s.expire_join_deadlines(at)?;
            Ok(())
        })
        .unwrap();
        let value = s.join_snapshot(&join).unwrap();
        assert_eq!(value.state, JoinState::Completed);
        assert_eq!(
            value.members[0].output.as_ref().unwrap().text,
            "human answer"
        );
        assert!(results(&s, "human").is_empty());
    }
}
#[test]
fn join_cancellation_precedes_delivery_but_stop_pause_and_failure_do_not_cancel() {
    let temp = crate::runtime::test_directory();
    let s = Store::open(&temp.path().join("db")).unwrap();
    let join = setup(&s, "requester", 1, None);
    for state in [AgentState::Stopped, AgentState::Failed] {
        s.atomic(|s| {
            let mut a = s.agent("requester")?;
            a.state = state;
            a.paused = true;
            s.save(&a)
        })
        .unwrap();
        assert_eq!(s.join_snapshot(&join).unwrap().state, JoinState::Collecting);
    }
    s.atomic(|s| {
        let (a, m) = consume(s, 0);
        say(s, &a, &m, "undelivered", "final_answer");
        s.message_state(&m, MessageState::Completed)?;
        let mut parent = s.agent("requester")?;
        parent.task_outcome = TaskOutcome::Cancelled;
        s.save(&parent)
    })
    .unwrap();
    assert_eq!(s.join_snapshot(&join).unwrap().state, JoinState::Cancelled);
    assert!(results(&s, "requester").is_empty());
}
#[test]
fn join_output_fences_selection_budget_and_recipient_purge() {
    let temp = crate::runtime::test_directory();
    let s = Store::open(&temp.path().join("db")).unwrap();
    let join = setup(&s, "requester", 1, None);
    let large = "🙂".repeat(MEMBER_BYTES / 4 + 50);
    s.atomic(|s| {
        let (mut a, m) = consume(s, 0);
        say(s, &a, &m, &large, "commentary");
        say(s, &a, &m, &format!("second {large}"), "commentary");
        say(s, &a, &m, &large, "final_answer");
        assert_eq!(join_artifact_count(s, &join), 1);
        // Event artifacts still belong to the worker and are never retired by the join.
        assert_eq!(
            s.query_row_cached(
                "SELECT count(*) FROM artifacts WHERE agent='worker-0'",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            3
        );
        say(s, &a, &m, "later commentary", "commentary");
        let execution = a.execution_id.replace("wrong".into());
        say(s, &a, &m, "wrong fence", "final_answer");
        a.execution_id = execution;
        s.message_state(&m, MessageState::Completed)?;
        assert!(s.joined_completion(&a, Some(&m.id), "Superseded task summary")?);
        assert!(s.joined_completion(&a, Some(&m.id), "Different task completion summary")?);
        assert_eq!(join_artifact_count(s, &join), 2);
        a.state = AgentState::Stopped;
        a.task_outcome = TaskOutcome::Completed;
        a.turn_id = None;
        a.conversation_clean = true;
        s.save(&a)?;
        Ok(())
    })
    .unwrap();
    let member = s.join_snapshot(&join).unwrap().members.remove(0);
    let output = member.output.unwrap();
    let task = member.task_completed.unwrap();
    assert_eq!(output.text.len() + task.text.len(), MEMBER_BYTES);
    assert!(output.text.ends_with('🙂'));
    assert_eq!(task.text, "");
    let artifact = output.read_target.unwrap();
    let task_artifact = task.read_target.unwrap();
    s.atomic(|s| s.purge("worker-0", false, false, &HashSet::default()))
        .unwrap();
    let first = s.events(&artifact, 0, 65536).unwrap();
    let second = s
        .events(&artifact, first["next_offset"].as_u64().unwrap(), 65536)
        .unwrap();
    let text = format!(
        "{}{}",
        first["text"].as_str().unwrap(),
        second["text"].as_str().unwrap()
    );
    assert!(text.contains(&large));
    assert!(
        s.events(&task_artifact, 0, 65536).unwrap()["text"]
            .as_str()
            .unwrap()
            .contains("Different task completion summary")
    );
    assert_eq!(s.join_snapshot(&join).unwrap().state, JoinState::Completed);
    s.atomic(|s| {
        let mut a = s.agent("requester")?;
        a.task_outcome = TaskOutcome::Cancelled;
        a.state = AgentState::Stopped;
        a.conversation_clean = true;
        s.save(&a)?;
        s.undeliverable("requester")?;
        s.purge("requester", false, false, &HashSet::default())
    })
    .unwrap();
    assert!(s.join_snapshot(&join).is_err());
    assert!(s.events(&artifact, 0, 100).is_err());
}
#[test]
fn join_rollback_guard_and_cancellation_purge_detach_surviving_members() {
    let temp = crate::runtime::test_directory();
    let s = Store::open(&temp.path().join("db")).unwrap();
    let join = setup(&s, "requester", 1, None);
    s.begin().unwrap();
    s.execute_cached(
        "UPDATE agents SET task_outcome='cancelled',ended_at=1 WHERE id='requester'",
        [],
    )
    .unwrap();
    s.settle_joins().unwrap();
    assert_eq!(s.join_snapshot(&join).unwrap().state, JoinState::Cancelled);
    s.rollback().unwrap();
    assert_eq!(s.join_snapshot(&join).unwrap().state, JoinState::Collecting);
    s.atomic(|s| {
        let mut a = s.agent("requester")?;
        a.task_outcome = TaskOutcome::Cancelled;
        a.state = AgentState::Stopped;
        a.conversation_clean = true;
        s.save(&a)?;
        s.undeliverable("requester")?;
        s.purge("requester", false, false, &HashSet::default())
    })
    .unwrap();
    assert!(!s.next_message("worker-0").unwrap().unwrap().joined);
    finish(&s, 0, MessageState::Completed, "survivor");
    assert!(s.join_snapshot(&join).is_err());
}
#[test]
fn join_scheduled_promotion_and_quarantine_keep_membership() {
    for corrupt in [false, true] {
        let temp = crate::runtime::test_directory();
        let s = Store::open(&temp.path().join("db")).unwrap();
        s.save(&agent("requester", None)).unwrap();
        s.save(&agent("worker-0", None)).unwrap();
        let join = s
            .atomic(|s| {
                let join = s.create_join("requester", "timer", None)?;
                s.enqueue_scheduled("timer-message", "requester", "worker-0", "later", 1)?;
                s.join_member(&join, "requester", "timer-message", 0)?;
                Ok(join)
            })
            .unwrap();
        assert!(
            s.scheduled_proof("timer-message", "requester")
                .unwrap()
                .valid
        );
        s.atomic(|s| {
            if corrupt {
                s.execute_cached(
                    "UPDATE deliveries SET error='bad' WHERE message_id='timer-message'",
                    [],
                )?;
            }
            let row = s.due_scheduled(1)?.remove(0);
            if corrupt {
                s.quarantine_scheduled(&row, false)?;
            } else {
                s.activate_scheduled(&row, MessageState::Accepted)?;
            }
            Ok(())
        })
        .unwrap();
        if corrupt {
            assert_eq!(
                s.join_snapshot(&join).unwrap().members[0].outcome,
                JoinOutcome::Failed
            );
        } else {
            finish(&s, 0, MessageState::Completed, "timer answer");
        }
        assert_eq!(s.join_snapshot(&join).unwrap().state, JoinState::Completed);
    }
}

#[test]
fn join_pending_text_is_withheld_at_deadline_and_continuation_cannot_replace_interruption() {
    let temp = crate::runtime::test_directory();
    let s = Store::open(&temp.path().join("db")).unwrap();
    let at = state::now() + 1;
    let join = setup(&s, "requester", 1, Some(at));
    let (a, m) = consume(&s, 0);
    say(&s, &a, &m, "partial unique", "commentary");
    s.atomic(|s| s.expire_join_deadlines(at)).unwrap();
    assert!(!results(&s, "requester")[0].contains("partial unique"));
    s.atomic(|s| s.message_state(&m, MessageState::Interrupted))
        .unwrap();
    let before = s.join_snapshot(&join).unwrap();
    assert_eq!(before.members[0].outcome, JoinOutcome::Interrupted);
    assert!(results(&s, "requester")[1].contains("partial unique"));
    // The normal auth fallback creates a new input rooted at this original message.
    s.enqueue("continuation", "human", &a.id, "continue task")
        .unwrap();
    s.execute_cached("UPDATE messages SET kind='continuation',continuation_root=?1,continuation_attempt=1 WHERE id='continuation'",[&m.id]).unwrap();
    s.assistant_output(
        &a,
        Some("continuation"),
        json!({"turn_id":"later-turn","text":"different result","phase":"final_answer"}),
        true,
    )
    .unwrap();
    s.atomic(|s| {
        let next = s.next_message(&a.id)?.unwrap();
        s.message_state(&next, MessageState::Completed)
    })
    .unwrap();
    assert_eq!(
        api::value(s.join_snapshot(&join).unwrap()),
        api::value(before)
    );
    assert_eq!(results(&s, "requester").len(), 2);
}
#[test]
fn join_outputs_survive_call_compaction_and_strict_snapshot_parsing_rejects_arrays() {
    let temp = crate::runtime::test_directory();
    let s = Store::open(&temp.path().join("db")).unwrap();
    let join = setup(&s, "human", 2, None);
    finish(&s, 0, MessageState::Completed, "retained answer");
    s.execute_cached("DELETE FROM call_results WHERE message_id='input-0'", [])
        .unwrap();
    assert_eq!(
        s.join_snapshot(&join).unwrap().members[0]
            .output
            .as_ref()
            .unwrap()
            .text,
        "retained answer"
    );
    s.execute_cached(
        "UPDATE join_members SET output='[\"text\",null]' WHERE message_id='input-0'",
        [],
    )
    .unwrap();
    assert_eq!(s.join_snapshot(&join).unwrap_err().code(), "database_error");
    assert_eq!(
        s.find_join_snapshot(&join).unwrap_err().code(),
        "database_error"
    );
    let request = api::Request::Read(api::ReadArgs {
        target: format!("join:{join}").into(),
        after: api::Input::Missing,
        limit: api::Input::Missing,
    });
    let config = crate::config::Config::parse("auth_profiles: {}\n").unwrap();
    let error = crate::runtime::read_api::dispatch(&s, &request, None, &config).unwrap_err();
    assert_eq!(error.code(), "database_error");
    assert!(!matches!(error, crate::error::Error::Input { .. }));
}

#[test]
fn join_overflow_replacement_rolls_back_and_never_retires_worker_artifacts() {
    let temp = crate::runtime::test_directory();
    let s = Store::open(&temp.path().join("db")).unwrap();
    let join = setup(&s, "human", 1, None);
    let text = "x".repeat(MEMBER_BYTES + 1);
    let (a, m) = s
        .atomic(|s| {
            let (a, m) = consume(s, 0);
            say(s, &a, &m, &text, "commentary");
            Ok((a, m))
        })
        .unwrap();
    let original = s.join_snapshot(&join).unwrap().members[0]
        .output
        .clone()
        .unwrap();
    assert!(
        s.atomic(|s| {
            say(s, &a, &m, &format!("new {text}"), "commentary");
            Err::<(), _>(Error::invalid_request("rollback replacement"))
        })
        .is_err()
    );
    assert_eq!(
        s.join_snapshot(&join).unwrap().members[0]
            .output
            .as_ref()
            .unwrap()
            .read_target,
        original.read_target
    );
    assert!(
        s.events(original.read_target.as_ref().unwrap(), 0, 100)
            .is_ok()
    );
    let worker_artifact: String = s
        .query_row_cached(
            "SELECT id FROM artifacts WHERE agent='worker-0' AND join_id IS NULL LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let forged = api::value(api::joins::JoinOutput {
        text: "old".into(),
        read_target: Some(format!("artifact:{worker_artifact}")),
    });
    let error = s
        .atomic(|s| {
            s.execute_cached(
                "UPDATE join_members SET output=?1 WHERE join_id=?2",
                params![forged.to_string(), join],
            )?;
            s.assistant_output(
                &a,
                Some(&m.id),
                json!({"turn_id":a.turn_id,"text":"new","phase":"final_answer"}),
                true,
            )
        })
        .unwrap_err();
    assert_eq!(error.code(), "database_error");
    assert!(
        s.events(&format!("artifact:{worker_artifact}"), 0, 100)
            .is_ok()
    );
    assert!(
        s.events(original.read_target.as_ref().unwrap(), 0, 100)
            .is_ok()
    );
    assert_eq!(join_artifact_count(&s, &join), 1);
}
