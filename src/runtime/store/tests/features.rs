//! Core admission holds preserve durable work while unrelated agents progress.
use super::*;
use crate::runtime::api::{CallOutcome, RequiredBuild};
use crate::runtime::auth_fallback::{Availability, AvailabilityState, Probe, WaitingReason};

fn bound(id: &str) -> Agent {
    let mut a = agent(id, None);
    a.comb = Some(crate::config::CombBinding {
        credential_file: "/absent/comb-credential.json".into(),
        reviewer: true,
    });
    a
}

#[test]
fn core_admission_skips_held_agents_before_limit_and_preserves_their_input() {
    let tmp = crate::runtime::test_directory();
    let mut c = core(tmp.path());
    for n in 0..130 {
        let id = format!("held-{n:03}");
        c.store().save(&bound(&id)).unwrap();
        c.store().enqueue(&id, "human", &id, "retained").unwrap();
    }
    c.store().save(&agent("unrelated", None)).unwrap();
    c.store().enqueue("go", "human", "unrelated", "go").unwrap();
    assert_eq!(c.ready_agents(128).unwrap(), ["unrelated"]);
    assert!(!c.store().has_ready().unwrap());
    for n in 0..130 {
        let id = format!("held-{n:03}");
        assert_eq!(
            c.store().next_message(&id).unwrap().unwrap().body,
            "retained"
        );
        assert!(!c.store().claimed(&id).unwrap());
    }
    for (sql, parameters) in [
        (queries::NEXT_READY_AGENT, &[0_i64][..]),
        (queries::HAS_READY_AGENT, &[][..]),
    ] {
        let mut statement = c
            .store()
            .connection
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap();
        let plan: Vec<String> = statement
            .query_map(rusqlite::params_from_iter(parameters), |r| r.get(3))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(
            plan.iter().any(|row| row.contains("agents_ready")),
            "{plan:?}"
        );
        assert!(
            plan.iter().all(|row| !row.contains("TEMP B-TREE")),
            "{plan:?}"
        );
    }
}

#[test]
fn core_deadline_queries_skip_held_schedules_and_branches_before_limit() {
    let tmp = crate::runtime::test_directory();
    let mut c = core(tmp.path());
    c.store().save(&agent("source", None)).unwrap();
    for n in 0..130 {
        let id = format!("held-{n:03}");
        c.store().save(&bound(&id)).unwrap();
        c.store()
            .enqueue_scheduled(&id, "human", &id, "retained", 1)
            .unwrap();
        c.store().track_call(&id).unwrap();
        // Saved native intent with an overdue materialization deadline.
        c.store().connection.execute("INSERT INTO branches(agent,source,source_thread,source_execution,source_turn,phase,deadline) VALUES(?1,'source','thread','execution','turn','ready_to_fork',1)", [&id]).unwrap();
    }
    c.store().save(&agent("unrelated", None)).unwrap();
    c.store()
        .enqueue_scheduled("go", "human", "unrelated", "go", 2)
        .unwrap();
    c.store().track_call("go").unwrap();
    c.store().save(&agent("unrelated-branch", None)).unwrap();
    c.store().connection.execute("INSERT INTO branches(agent,source,source_thread,source_execution,source_turn,phase,deadline) VALUES('unrelated-branch','source','thread','execution','turn','ready_to_fork',2)", []).unwrap();
    assert_eq!(c.store().next_deadline().unwrap(), Some(2));
    assert_eq!(c.promote_scheduled(100).unwrap(), 1);
    assert_eq!(c.store().settle_branches(100).unwrap(), 1);
    assert_eq!(c.store().settle_branches(100).unwrap(), 0);
    assert_eq!(c.store().next_deadline().unwrap(), None);
    assert_eq!(c.promote_scheduled(100).unwrap(), 0);
    assert_eq!(
        c.store().next_message("unrelated").unwrap().unwrap().id,
        "go"
    );
    assert_eq!(
        c.store().message_result("held-000").unwrap().outcome,
        CallOutcome::Held
    );
    assert_eq!(
        c.store().message_result("held-000").unwrap().delivery_state,
        MessageState::Scheduled
    );
    assert!(
        c.store()
            .branch_intent("held-000")
            .unwrap()
            .unwrap()
            .error
            .is_none()
    );
}

#[test]
fn core_auth_candidates_skip_held_agents_but_keep_shared_probes() {
    let store = crate::runtime::test_support::TestStore::new();
    for n in 0..131 {
        let id = format!("agent-{n:03}");
        let mut a = if n < 130 {
            bound(&id)
        } else {
            agent(&id, None)
        };
        a.auth = id.clone();
        a.auth_group = Some("group".into());
        store.save(&a).unwrap();
        let mut availability = Availability::available(&a.auth, a.auth_generation);
        availability.state = AvailabilityState::Unavailable;
        availability.probe = Some(Probe {
            id: id.clone(),
            agent: id.clone(),
            message: id.clone(),
            deadline: 1,
            error: None,
        });
        store.save_revised(&mut availability).unwrap();
    }
    assert_eq!(store.affected_auth_agents("[]").unwrap(), ["agent-130"]);
    assert_eq!(store.probe_candidates(100).unwrap().len(), 128);
    let mut free = store
        .availability(
            "agent-130",
            store.agent("agent-130").unwrap().auth_generation,
        )
        .unwrap();
    free.clear_probe();
    store.save_revised(&mut free).unwrap();
    assert_eq!(store.probe_candidates(100).unwrap().len(), 128);
    assert_eq!(store.next_deadline().unwrap(), Some(1));
    for reason in [WaitingReason::Quota, WaitingReason::Switch] {
        for n in 0..131 {
            let mut a = store.agent(&format!("agent-{n:03}")).unwrap();
            a.state = AgentState::Stopped;
            a.waiting_reason = Some(reason);
            a.quota_wait_until = Some(1);
            store.save(&a).unwrap();
        }
        let candidates = if reason == WaitingReason::Quota {
            store.quota_waiters("{}").unwrap()
        } else {
            store.auth_switches_after("").unwrap()
        };
        assert_eq!(candidates, ["agent-130"]);
    }
    assert_eq!(store.probe_candidates(100).unwrap().len(), 128);
}

#[test]
fn core_join_deadlines_follow_requester_and_terminal_results_win() {
    use crate::runtime::api::joins::JoinState;
    let store = crate::runtime::test_support::TestStore::new();
    store.save(&bound("held-requester")).unwrap();
    store.save(&agent("requester", None)).unwrap();
    store.save(&bound("target")).unwrap();
    let mut joins = Vec::new();
    let deadline = state::now() + 1;
    for requester in ["held-requester", "requester", "human"] {
        let join = store
            .create_join(requester, "round", Some(deadline))
            .unwrap();
        store
            .enqueue(requester, requester, "target", "work")
            .unwrap();
        store.join_member(&join, requester, requester, 0).unwrap();
        joins.push(join);
    }
    assert_eq!(store.expire_join_deadlines(deadline).unwrap(), 2);
    assert_eq!(store.expire_join_deadlines(deadline).unwrap(), 0);
    assert_eq!(store.next_deadline().unwrap(), None);
    store.settle_joins().unwrap(); // Held dirty rows must not cause a settlement loop.
    for (index, join) in joins.iter().enumerate() {
        let snapshot = store.join_snapshot(join).unwrap();
        assert_eq!(
            snapshot.state,
            if index == 0 {
                JoinState::Collecting
            } else {
                JoinState::DeadlinePassed
            }
        );
        assert_eq!(snapshot.held_for_build, Some(RequiredBuild::Comb));
        assert_eq!(
            snapshot.members[0].held_for_build,
            Some(RequiredBuild::Comb)
        );
    }
    assert_eq!(
        store.agent_hold("requester").unwrap(),
        Some(RequiredBuild::Comb)
    );
    store.undeliverable("target").unwrap();
    store.settle_joins().unwrap();
    assert_eq!(
        store.join_snapshot(&joins[2]).unwrap().state,
        JoinState::Completed
    );
    assert_eq!(store.join_snapshot(&joins[2]).unwrap().held_for_build, None);
    assert_eq!(
        store.message_result("human").unwrap().outcome,
        CallOutcome::Cancelled
    );
    assert_eq!(store.message_result("human").unwrap().held_for_build, None);
    assert_eq!(
        store.join_snapshot(&joins[0]).unwrap().state,
        JoinState::Collecting
    );
}

#[test]
fn core_holds_are_live_observations_and_do_not_erase_crash_uncertainty() {
    let store = crate::runtime::test_support::TestStore::new();
    store.save(&bound("target")).unwrap();
    store.enqueue("input", "human", "target", "work").unwrap();
    store.track_call("input").unwrap();
    let receipt = json!({"results":[{"message_id":"input"}]});
    store.remember("human", "request", "{}", &receipt).unwrap();
    let mut observation = store.prior("human", "request", "{}").unwrap().unwrap();
    store.decorate_holds(&mut observation).unwrap();
    assert_eq!(observation["results"][0]["held_for_build"], "comb");
    let opaque = json!({"body":{"message_id":"input"}});
    let mut body = opaque.clone();
    store.decorate_holds(&mut body).unwrap();
    assert_eq!(body, opaque);
    assert_eq!(
        store.prior("human", "request", "{}").unwrap().unwrap(),
        receipt
    );
    let input = store.next_message("target").unwrap().unwrap();
    store
        .message_state(&input, MessageState::Submitted)
        .unwrap();
    store.recover_agents().unwrap();
    assert_eq!(
        store.message_result("input").unwrap().outcome,
        CallOutcome::Uncertain
    );
    assert_eq!(store.message_result("input").unwrap().held_for_build, None);
    let mut after = receipt.clone();
    store.decorate_holds(&mut after).unwrap();
    assert_eq!(after, receipt);
    assert!(store.agent("target").unwrap().comb.is_some());
}

#[test]
fn core_shared_probe_expiry_unblocks_other_agents_without_touching_held_owner() {
    let tmp = crate::runtime::test_directory();
    let c = core(tmp.path());
    let mut owner = bound("owner");
    owner.auth_group = Some("group".into());
    owner.auth_probe_id = Some("probe".into());
    owner.conversation_clean = false;
    c.store().save(&owner).unwrap();
    c.store()
        .enqueue("old", "human", "owner", "retained")
        .unwrap();
    let input = c.store().next_message("owner").unwrap().unwrap();
    c.store()
        .message_state(&input, MessageState::Uncertain)
        .unwrap();
    let mut waiter = agent("waiter", None);
    waiter.auth_group = Some("group".into());
    c.store().save(&waiter).unwrap();
    c.store().enqueue("new", "human", "waiter", "go").unwrap();
    let mut availability = Availability::available(&owner.auth, owner.auth_generation);
    availability.retry_at = Some(1);
    availability.probe = Some(Probe {
        id: "probe".into(),
        agent: owner.id.clone(),
        message: "old".into(),
        deadline: 1,
        error: None,
    });
    c.store().save_revised(&mut availability).unwrap();
    let snapshot = || {
        let data: String = c
            .store()
            .connection
            .query_row("SELECT data FROM agents WHERE id='owner'", [], |r| r.get(0))
            .unwrap();
        (
            data,
            c.store().probe_input("owner", "old").unwrap(),
            c.store().claimed("owner").unwrap(),
        )
    };
    let before = snapshot();
    assert!(!availability.usable(state::now(), Some("waiter")));
    assert_eq!(c.store().next_deadline().unwrap(), Some(1));
    assert_eq!(c.reconcile_auth_probes(100).unwrap(), 1);
    assert_eq!(c.reconcile_auth_probes(100).unwrap(), 0);
    assert_eq!(c.store().next_deadline().unwrap(), None);
    assert_eq!(snapshot(), before);
    let input = c.store().next_message("waiter").unwrap().unwrap();
    assert!(c.auth_admit(&mut waiter, &input).unwrap());
    assert_eq!(
        c.store()
            .availability(&owner.auth, owner.auth_generation)
            .unwrap()
            .probe_agent(),
        Some("waiter")
    );
    assert_eq!(snapshot(), before);
}

#[test]
fn core_ended_bound_requesters_cancel_joins_and_preserve_independent_work() {
    use crate::runtime::{
        api::{RequestEnvelope, joins::JoinState},
        caller::Caller,
        preflight::request::Facts,
    };
    for expired in [false, true] {
        let tmp = crate::runtime::test_directory();
        let mut c = core(tmp.path());
        c.store().save(&agent("parent", None)).unwrap();
        let mut requester = bound("requester");
        requester.parent_id = Some("parent".into());
        requester.lifetime = crate::config::Lifetime::WithParent;
        c.store().save(&requester).unwrap();
        c.store().save(&agent("target", None)).unwrap();
        c.store()
            .enqueue("work", "requester", "target", "independent")
            .unwrap();
        let join = c
            .store()
            .create_join("requester", "round", Some(state::now() + 1))
            .unwrap();
        c.store()
            .join_member(&join, "requester", "work", 0)
            .unwrap();
        c.store().settle_joins().unwrap();
        assert_eq!(
            c.store().join_snapshot(&join).unwrap().held_for_build,
            Some(RequiredBuild::Comb)
        );
        let request = RequestEnvelope::new(
            json!({"op":"terminate", "target":if expired {"parent"} else {"requester"}}),
        );
        c.atomic(|core| core.dispatch(&Caller::Human, "end", &request, &Facts::default()))
            .unwrap();
        let snapshot = c.store().join_snapshot(&join).unwrap();
        assert_eq!(snapshot.state, JoinState::Cancelled);
        assert_eq!(snapshot.held_for_build, None);
        assert_eq!(c.store().next_deadline().unwrap(), None);
        assert_eq!(
            c.store().next_message("target").unwrap().unwrap().body,
            "independent"
        );
    }
}

#[test]
fn core_source_status_explains_held_branch_until_the_hold_is_released() {
    use crate::runtime::{
        api::RequestEnvelope,
        branching::{BranchFailure, BranchPhase},
    };
    let store = crate::runtime::test_support::TestStore::new();
    let mut source = agent("source", None);
    source.profile.provider = crate::config::Provider::Claude;
    store.save(&source).unwrap();
    let mut child = bound("branch");
    child.profile.provider = crate::config::Provider::Claude;
    child.branch_phase = Some(BranchPhase::ReadyToFork);
    store.save(&child).unwrap();
    store.connection.execute("INSERT INTO branches(agent,source,source_thread,source_execution,source_turn,phase,deadline) VALUES('branch','source','thread','execution','turn','ready_to_fork',1)", []).unwrap();
    let status = || {
        crate::runtime::read_api::dispatch(
            &store,
            &RequestEnvelope::new(json!({"op":"status","target":"source"})).request,
            None,
            &Config::parse("auth_profiles: {}\nroles: {}\n").unwrap(),
        )
        .unwrap()
    };
    assert_eq!(
        store.agent_hold("source").unwrap(),
        Some(RequiredBuild::Comb)
    );
    assert_eq!(status()["agent"]["held_for_build"], "comb");
    store.enqueue("queued", "human", "source", "later").unwrap();
    store.track_call("queued").unwrap();
    assert_eq!(
        store.message_result("queued").unwrap().outcome,
        CallOutcome::Held
    );
    store.save(&agent("requester", None)).unwrap();
    store
        .enqueue("member", "requester", "source", "joined")
        .unwrap();
    let join = store.create_join("requester", "round", None).unwrap();
    store.join_member(&join, "requester", "member", 0).unwrap();
    assert_eq!(
        store.agent_hold("requester").unwrap(),
        Some(RequiredBuild::Comb)
    );
    // Waiting on a join is observable, but does not prevent independent input.
    assert_eq!(store.recipient_hold("requester").unwrap(), None);
    let acceptance = json!({"message_id":"queued"});
    let mut held = acceptance.clone();
    store.decorate_holds(&mut held).unwrap();
    assert_eq!(held["held_for_build"], "comb");
    store
        .fail_branch(&mut child, BranchFailure::BranchTerminated, false)
        .unwrap();
    assert_eq!(store.agent_hold("source").unwrap(), None);
    assert!(status()["agent"].get("held_for_build").is_none());
    assert_eq!(
        store.message_result("queued").unwrap().outcome,
        CallOutcome::Pending
    );
    let mut released = acceptance.clone();
    store.decorate_holds(&mut released).unwrap();
    assert_eq!(released, acceptance);
    assert_eq!(store.agent_hold("requester").unwrap(), None);
}
