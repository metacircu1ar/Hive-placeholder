use super::*;
use crate::runtime::branching::{AttemptedTurn, BranchPhase, FinishedTurn};

fn source(store: &Store) -> Agent {
    let mut a = agent("source", None);
    a.thread_id = Some("source-thread".into());
    a.execution_id = Some("source-execution".into());
    a.conversation_clean = true;
    a.state = AgentState::Stopped;
    store.save(&a).unwrap();
    store
        .enqueue("prefix", "human", &a.id, "secret-prefix")
        .unwrap();
    finish(store, &mut a, "prefix", "turn-prefix");
    a
}
fn finish(store: &Store, a: &mut Agent, message: &str, turn: &str) {
    store
        .turn_started(
            a,
            crate::runtime::api::events::TurnStarted {
                attachments: &[],
                turn_id: turn,
                message_id: message,
                sender: "human",
                selection: crate::runtime::api::events::TurnSelection {
                    model: &a.model,
                    effort: &a.effort,
                    options_revision: a.options_revision,
                    selection_verification: "requested_unverified",
                },
            },
        )
        .unwrap();
    let input = store.next_message(&a.id).unwrap();
    if let Some(input) = input.filter(|m| m.id == message) {
        store
            .message_state(&input, MessageState::Completed)
            .unwrap();
    } else {
        store
            .connection
            .execute(
                "UPDATE messages SET state='completed' WHERE id=?1",
                [message],
            )
            .unwrap();
    }
    a.last_attempted_turn = Some(AttemptedTurn {
        message: message.into(),
        thread: a.thread_id.clone().unwrap(),
        execution: a.execution_id.clone().unwrap(),
    });
    a.last_finished_turn = Some(FinishedTurn {
        thread: a.thread_id.clone().unwrap(),
        execution: a.execution_id.clone().unwrap(),
        turn: turn.into(),
        message: Some(message.into()),
        outcome: crate::runtime::domain::ProviderTurnStatus::Completed,
        at: state::now(),
    });
    a.turn_id = None;
    store.save(a).unwrap();
}
fn install(store: &Store, source: &Agent, id: &str) -> Agent {
    let child = agent(id, None);
    store.save(&child).unwrap();
    store
        .enqueue(&format!("input-{id}"), "human", id, "new task")
        .unwrap();
    store
        .install_branch(&child, &store.branch_boundary(source).unwrap())
        .unwrap();
    store.agent(id).unwrap()
}

#[test]
fn boundary_uses_submission_order_when_redirect_reorders_the_inbox() {
    let tmp = crate::runtime::test_directory();
    let store = Store::open(&tmp.path().join("db")).unwrap();
    let mut a = source(&store);
    store
        .enqueue("older", "human", &a.id, "older queued input")
        .unwrap();
    store
        .enqueue("redirect", "human", &a.id, "newer redirected input")
        .unwrap();
    finish(&store, &mut a, "redirect", "redirect-turn");
    a.last_attempted_turn = Some(AttemptedTurn {
        message: "older".into(),
        thread: "source-thread".into(),
        execution: "source-execution".into(),
    });
    store
        .connection
        .execute("UPDATE messages SET state='uncertain' WHERE id='older'", [])
        .unwrap();
    store.save(&a).unwrap();
    assert_eq!(store.branch_boundary(&a).unwrap_err().code(), "agent_busy");
    finish(&store, &mut a, "older", "older-turn");
    assert_eq!(store.branch_boundary(&a).unwrap().source_turn, "older-turn");
}

#[test]
fn source_hold_survives_stale_saves_and_multiple_branches_release_independently() {
    let tmp = crate::runtime::test_directory();
    let store = Store::open(&tmp.path().join("db")).unwrap();
    let a = source(&store);
    install(&store, &a, "one");
    install(&store, &a, "two");
    store.save(&a).unwrap();
    assert_eq!(store.agent(&a.id).unwrap().branch_hold, 2);
    store.settle_branches(state::now()).unwrap();
    for (id, count) in [("one", 1), ("two", 0)] {
        let mut child = store.agent(id).unwrap();
        assert_eq!(child.branch_phase, Some(BranchPhase::ReadyToFork));
        store.begin_branch(&mut child).unwrap();
        child.execution_id = Some(format!("execution-{id}"));
        store.save(&child).unwrap();
        store
            .bind_branch(&mut child, &format!("thread-{id}"))
            .unwrap();
        assert_eq!(store.agent(&a.id).unwrap().branch_hold, count);
    }
}

#[test]
fn branch_handoff_cannot_introduce_queued_or_other_conversation_context() {
    let tmp = crate::runtime::test_directory();
    let store = Store::open(&tmp.path().join("db")).unwrap();
    let a = source(&store);
    store
        .enqueue("future", "human", &a.id, "must-not-leak-future")
        .unwrap();
    store
        .enqueue("other-chat", "human", &a.id, "must-not-leak-other-chat")
        .unwrap();
    store
        .connection
        .execute(
            "UPDATE messages SET state='completed' WHERE id='other-chat'",
            [],
        )
        .unwrap();
    store.connection.execute("UPDATE deliveries SET thread='another-thread',recipient_turn='other-turn' WHERE message_id='other-chat'",[]).unwrap();
    install(&store, &a, "child");
    store.settle_branches(state::now()).unwrap();
    let handoff = store
        .branch_intent("child")
        .unwrap()
        .unwrap()
        .source_handoff
        .unwrap();
    assert!(handoff.contains("secret-prefix"));
    assert!(!handoff.contains("must-not-leak"), "{handoff}");
}

#[test]
fn active_boundary_deadline_starts_at_completion_and_ambiguous_creation_never_replays() {
    let tmp = crate::runtime::test_directory();
    let store = Store::open(&tmp.path().join("db")).unwrap();
    let mut a = source(&store);
    a.turn_id = Some("active-turn".into());
    a.state = AgentState::Running;
    store.save(&a).unwrap();
    install(&store, &a, "child");
    assert!(
        store
            .branch_intent("child")
            .unwrap()
            .unwrap()
            .deadline
            .is_none()
    );
    assert_eq!(store.settle_branches(state::now() + 1000).unwrap(), 0);
    store
        .enqueue("active-input", "human", &a.id, "active boundary")
        .unwrap();
    finish(&store, &mut a, "active-input", "active-turn");
    a.state = AgentState::Stopped;
    store.save(&a).unwrap();
    store.settle_branches(state::now()).unwrap();
    let intent = store.branch_intent("child").unwrap().unwrap();
    assert_eq!(intent.phase, BranchPhase::ReadyToFork);
    let mut child = store.agent("child").unwrap();
    store.begin_branch(&mut child).unwrap();
    store.recover_agents().unwrap();
    store.settle_branches(state::now()).unwrap();
    let mut child = store.agent("child").unwrap();
    assert_eq!(child.branch_phase, Some(BranchPhase::Uncertain));
    assert!(store.begin_branch(&mut child).is_err());
    assert_eq!(store.agent(&a.id).unwrap().branch_hold, 1);
    store.settle_branches(intent.deadline.unwrap()).unwrap();
    assert_eq!(store.agent(&a.id).unwrap().branch_hold, 0);
    assert!(store.bind_branch(&mut child, "late-child-thread").is_err());
    assert_eq!(
        store.agent("child").unwrap().branch_phase,
        Some(BranchPhase::Uncertain)
    );
}

#[test]
fn pending_branches_protect_source_from_purge_and_batch_rollback_removes_holds() {
    let tmp = crate::runtime::test_directory();
    let store = Store::open(&tmp.path().join("db")).unwrap();
    let mut a = source(&store);
    store.begin().unwrap();
    install(&store, &a, "rolled-back");
    store.rollback().unwrap();
    assert_eq!(store.agent(&a.id).unwrap().branch_hold, 0);
    assert!(store.agent("rolled-back").is_err());
    install(&store, &a, "child");
    a.task_outcome = TaskOutcome::Completed;
    store.save(&a).unwrap();
    let preview = store.purge(&a.id, false, true, &HashSet::new()).unwrap();
    assert_eq!(
        preview.blockers[0].reason,
        crate::runtime::domain::PurgeBlockerReason::PendingNativeBranches
    );
    let mut child = store.agent("child").unwrap();
    store
        .fail_branch(
            &mut child,
            crate::runtime::branching::BranchFailure::BranchCancelled,
            false,
        )
        .unwrap();
    assert_eq!(
        store
            .purge(&a.id, false, true, &HashSet::new())
            .unwrap()
            .blocked_agents,
        0
    );
}

#[test]
fn a_reservation_is_not_a_turn_but_unacknowledged_native_input_blocks_capture() {
    let tmp = crate::runtime::test_directory();
    let store = Store::open(&tmp.path().join("db")).unwrap();
    let mut a = source(&store);
    store
        .enqueue("queued", "human", &a.id, "next turn")
        .unwrap();
    store.reserve_turn(&mut a).unwrap();
    store.save(&a).unwrap();
    assert_eq!(
        store.branch_boundary(&a).unwrap().source_turn,
        "turn-prefix"
    );
    let message = store.next_message(&a.id).unwrap().unwrap();
    store
        .message_state(&message, MessageState::Submitting)
        .unwrap();
    assert_eq!(store.branch_boundary(&a).unwrap_err().code(), "agent_busy");
}

#[test]
fn precreation_rejection_is_retryable_but_late_native_identity_does_not_reopen_abandonment() {
    let tmp = crate::runtime::test_directory();
    let store = Store::open(&tmp.path().join("db")).unwrap();
    let a = source(&store);
    install(&store, &a, "child");
    store.settle_branches(state::now()).unwrap();
    let mut child = store.agent("child").unwrap();
    store.begin_branch(&mut child).unwrap();
    assert!(
        store
            .branch_creation_rejected(
                &mut child,
                crate::runtime::branching::BranchFailure::ForkNotSubmitted
            )
            .unwrap()
    );
    assert_eq!(child.branch_phase, Some(BranchPhase::ReadyToFork));
    store.begin_branch(&mut child).unwrap();
    store
        .fail_branch(
            &mut child,
            crate::runtime::branching::BranchFailure::BranchCreationUncertain,
            false,
        )
        .unwrap();
    store
        .record_branch_native(&child, "late-known-thread")
        .unwrap();
    let intent = store.branch_intent("child").unwrap().unwrap();
    assert_eq!(intent.phase, BranchPhase::Uncertain);
    assert_eq!(intent.native_thread.as_deref(), Some("late-known-thread"));
    assert!(
        store
            .branch_creation_rejected(
                &mut child,
                crate::runtime::branching::BranchFailure::ForkNotSubmitted
            )
            .is_err()
    );
    assert!(store.begin_branch(&mut child).is_err());
}

#[test]
fn materialization_timeout_retains_the_first_precreation_rejection() {
    let tmp = crate::runtime::test_directory();
    let store = Store::open(&tmp.path().join("db")).unwrap();
    let a = source(&store);
    install(&store, &a, "child");
    store.settle_branches(state::now()).unwrap();
    let mut child = store.agent("child").unwrap();
    store.begin_branch(&mut child).unwrap();
    assert!(
        store
            .branch_creation_rejected(
                &mut child,
                crate::runtime::branching::BranchFailure::ForkNotSubmitted
            )
            .unwrap()
    );
    let deadline = store
        .branch_intent("child")
        .unwrap()
        .unwrap()
        .deadline
        .unwrap();
    store.settle_branches(deadline).unwrap();
    let intent = store.branch_intent("child").unwrap().unwrap();
    assert_eq!(intent.phase, BranchPhase::Failed);
    assert_eq!(
        intent.error.as_deref(),
        Some("fork_not_submitted; branch_materialization_timeout")
    );
    assert_eq!(store.agent("source").unwrap().branch_hold, 0);
}

#[test]
fn consistent_terminal_branch_races_are_busy_and_preserve_each_failure_cause_once() {
    let tmp = crate::runtime::test_directory();
    let store = Store::open(&tmp.path().join("db")).unwrap();
    let a = source(&store);
    install(&store, &a, "child");
    store.settle_branches(state::now()).unwrap();
    let mut child = store.agent("child").unwrap();
    store.begin_branch(&mut child).unwrap();
    assert_eq!(store.fork_intent(&child).unwrap_err().code(), "agent_busy");
    assert!(
        store
            .branch_creation_rejected(
                &mut child,
                crate::runtime::branching::BranchFailure::ForkNotSubmitted
            )
            .unwrap()
    );
    store.begin_branch(&mut child).unwrap();
    store
        .fail_branch(
            &mut child,
            crate::runtime::branching::BranchFailure::Error(
                crate::error::ErrorCode::ProviderTimeout,
            ),
            false,
        )
        .unwrap();
    assert_eq!(store.fork_intent(&child).unwrap_err().code(), "agent_busy");
    store
        .fail_branch(
            &mut child,
            crate::runtime::branching::BranchFailure::Error(
                crate::error::ErrorCode::ProviderTimeout,
            ),
            false,
        )
        .unwrap();
    assert_eq!(
        store
            .branch_intent("child")
            .unwrap()
            .unwrap()
            .error
            .as_deref(),
        Some("fork_not_submitted; provider_timeout")
    );
    install(&store, &a, "cancelled");
    let mut cancelled = store.agent("cancelled").unwrap();
    store
        .fail_branch(
            &mut cancelled,
            crate::runtime::branching::BranchFailure::BranchCancelled,
            true,
        )
        .unwrap();
    assert_eq!(
        store.fork_intent(&cancelled).unwrap_err().code(),
        "agent_busy"
    );
}

#[test]
fn missing_or_inconsistent_intent_cannot_fall_through_to_fresh_creation() {
    let tmp = crate::runtime::test_directory();
    let store = Store::open(&tmp.path().join("db")).unwrap();
    let a = source(&store);
    install(&store, &a, "child");
    store.settle_branches(state::now()).unwrap();
    let mut child = store.agent("child").unwrap();
    store
        .connection
        .execute("DELETE FROM branches WHERE agent='child'", [])
        .unwrap();
    assert!(store.fork_intent(&child).is_err());
    assert!(store.begin_branch(&mut child).is_err());
    assert!(store.fork_intent(&store.agent("child").unwrap()).is_err());
}

#[test]
fn restart_of_unfinished_source_fails_capture_instead_of_using_older_history() {
    let tmp = crate::runtime::test_directory();
    let store = Store::open(&tmp.path().join("db")).unwrap();
    let mut a = source(&store);
    a.turn_id = Some("unfinished".into());
    a.state = AgentState::Running;
    store.save(&a).unwrap();
    install(&store, &a, "child");
    store.recover_agents().unwrap();
    store.settle_branches(state::now()).unwrap();
    let intent = store.branch_intent("child").unwrap().unwrap();
    assert_eq!(intent.phase, BranchPhase::Failed);
    assert_eq!(intent.error.as_deref(), Some("source_boundary_uncertain"));
    assert_eq!(store.agent("source").unwrap().branch_hold, 0);
}

#[test]
fn branch_quota_parks_and_wakes_only_its_pinned_generation_and_deadline_still_expires() {
    use crate::runtime::auth_fallback::{AvailabilityState, WaitingReason};
    let tmp = crate::runtime::test_directory();
    let core = core(tmp.path());
    let a = source(core.store());
    install(core.store(), &a, "child");
    core.store().settle_branches(state::now()).unwrap();
    let mut child = core.store().agent("child").unwrap();
    core.store()
        .ensure_availability(&child.auth, child.auth_generation)
        .unwrap();
    let mut value = core
        .store()
        .availability(&child.auth, child.auth_generation)
        .unwrap();
    value.state = AvailabilityState::Limited;
    value.until = Some(state::now() + 3600);
    value.reason = Some(crate::runtime::auth_fallback::AuthReason::UsageLimit);
    core.store().save_availability(&value).unwrap();
    let message = core.store().next_message(&child.id).unwrap().unwrap();
    assert!(!core.auth_admit(&mut child, &message).unwrap());
    assert_eq!(child.waiting_reason, Some(WaitingReason::Quota));
    assert!(child.auth_switch_pending.is_none());
    assert!(!core.unpark_auth_waiter(&mut child).unwrap());
    value.state = AvailabilityState::Available;
    value.until = None;
    core.store().save_availability(&value).unwrap();
    assert!(core.unpark_auth_waiter(&mut child).unwrap());
    assert!(child.waiting_reason.is_none());
    assert_eq!(child.branch_phase, Some(BranchPhase::ReadyToFork));
    value.state = AvailabilityState::Limited;
    value.until = Some(state::now() + 3600);
    core.store().save_availability(&value).unwrap();
    assert!(!core.auth_admit(&mut child, &message).unwrap());
    let deadline = core
        .store()
        .branch_intent(&child.id)
        .unwrap()
        .unwrap()
        .deadline
        .unwrap();
    core.store().settle_branches(deadline).unwrap();
    assert_eq!(
        core.store().agent(&child.id).unwrap().branch_phase,
        Some(BranchPhase::Failed)
    );
    assert_eq!(core.store().agent("source").unwrap().branch_hold, 0);
    assert!(
        !core
            .store()
            .events(&child.id, 0, 200)
            .unwrap()
            .to_string()
            .contains("auth_switch")
    );
}

#[test]
fn branch_settings_use_effective_selection_and_explicit_auth_can_drop_the_group() {
    let config = Config::parse("auth_profiles:\n  main: {provider: codex, method: subscription}\nauth_groups: {plans: {mode: ordered, members: [main]}}\nroles: {}\n").unwrap();
    let mut a = agent("source", None);
    a.auth_group = Some("plans".into());
    a.model = "future-model".into();
    a.effort = "high".into();
    a.turn_selection = Some(crate::runtime::domain::OptionsSelection {
        options: crate::options::Snapshot {
            values: [
                ("model".into(), json!("active-model")),
                ("effort".into(), json!("low")),
            ]
            .into(),
            sources: std::collections::BTreeMap::default(),
        }
        .into(),
        model: "active-model".into(),
        effort: "low".into(),
        revision: 1,
    });
    let source = crate::runtime::branching::Source {
        agent: a,
        instructions: "saved".into(),
    };
    let args: crate::runtime::api::SpawnArgs =
        serde_json::from_value(json!({"body":"new"})).unwrap();
    let inherited = source.resolve(&config, &args).unwrap();
    assert_eq!(inherited.model, "active-model");
    assert_eq!(inherited.effort, "low");
    assert_eq!(inherited.auth, "plans");
    let explicit: crate::runtime::api::SpawnArgs =
        serde_json::from_value(json!({"body":"new","auth":"main"})).unwrap();
    assert_eq!(source.resolve(&config, &explicit).unwrap().auth, "main");
}

#[test]
fn branch_deadlines_reject_unrepresentable_input_and_corrupt_negative_storage() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    let source = source(&store);
    let child = agent("child", None);
    store.save(&child).unwrap();
    let mut intent = store.branch_boundary(&source).unwrap();
    intent.deadline = Some(u64::MAX);
    assert_eq!(
        store.install_branch(&child, &intent).unwrap_err().code(),
        "database_error"
    );
    assert!(store.branch_intent("child").unwrap().is_none());
    assert_eq!(store.agent("source").unwrap().branch_hold, 0);
    assert_eq!(store.agent("child").unwrap().branch_phase, None);
    intent.deadline = Some(i64::MAX.unsigned_abs());
    store.install_branch(&child, &intent).unwrap();
    assert_eq!(
        store.branch_intent("child").unwrap().unwrap().deadline,
        intent.deadline
    );
    assert_eq!(
        store.settle_branches(u64::MAX).unwrap_err().code(),
        "database_error"
    );
    assert_eq!(
        store.branch_intent("child").unwrap().unwrap().deadline,
        intent.deadline
    );
    store
        .connection
        .pragma_update(None, "ignore_check_constraints", true)
        .unwrap();
    store
        .connection
        .execute("UPDATE branches SET deadline=-1 WHERE agent='child'", [])
        .unwrap();
    store
        .connection
        .pragma_update(None, "ignore_check_constraints", false)
        .unwrap();
    assert_eq!(
        store.branch_intent("child").unwrap_err().code(),
        "database_error"
    );
}

#[test]
fn attachments_image_history_budget_is_inherited_without_sharing_ownership() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    let source = source(&store);
    store
        .execute_cached(
            "INSERT INTO agent_image_usage(agent,bytes,images) VALUES ('source',1024,20)",
            [],
        )
        .unwrap();
    let child = install(&store, &source, "child");
    assert_eq!(
        store.image_usage(&child.id).unwrap(),
        store.image_usage(&source.id).unwrap()
    );
    store
        .execute_cached("DELETE FROM agent_image_usage WHERE agent='source'", [])
        .unwrap();
    assert_eq!(store.image_usage(&child.id).unwrap().images, 20);
}
