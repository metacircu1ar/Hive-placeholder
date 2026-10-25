use super::*;
use crate::runtime::{
    api::joins::{JoinOutcome, JoinState},
    domain::MessageState,
};

fn request(operations: Value) -> Value {
    json!({"op":"batch","join":{},"operations":operations})
}
fn send(id: &str, target: &str) -> Value {
    json!({"op":"send","target":target,"body":"Return final answer","request_id":id})
}
fn configured(c: &mut Core) {
    let profile: Profile =
        serde_json::from_value(json!({"provider":"codex","method":"subscription"})).unwrap();
    c.config_mut()
        .auth_profiles
        .insert("main".into(), profile.clone());
    c.config_mut().roles.insert(
        "test".into(),
        serde_json::from_value(json!({"auth":"main","model":"mock"})).unwrap(),
    );
    c.state().register("main", &profile).unwrap();
}
#[test]
fn joined_batch_atomic_replay_duplicate_targets_and_fresh_ids() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    for id in ["caller", "a"] {
        c.store().save(&agent(id)).unwrap();
    }
    let broken = request(json!([send("first", "a"), send("second", "missing")]));
    assert!(call(&mut c, "caller", "round", broken).is_err());
    assert!(c.store().open_joins("caller").unwrap().is_empty());
    assert!(!c.store().request_used("caller", "first").unwrap());
    assert!(c.store().next_message("a").unwrap().is_none());
    let good = request(json!([send("first", "a"), send("second", "a")]));
    let accepted = call(&mut c, "caller", "round", good.clone()).unwrap();
    assert_eq!(accepted["join"]["members"].as_array().unwrap().len(), 2);
    assert_ne!(
        accepted["join"]["members"][0],
        accepted["join"]["members"][1]
    );
    assert_eq!(
        call(&mut c, "caller", "round", good.clone()).unwrap(),
        accepted
    );
    assert!(call(&mut c, "caller", "other-round", good).is_err());
    assert!(
        call(
            &mut c,
            "caller",
            "dupe",
            request(json!([send("third", "a"), send("third", "a")]))
        )
        .is_err()
    );
    assert!(!c.store().request_used("caller", "third").unwrap());
    let id = accepted["join"]["join_id"].as_str().unwrap();
    assert_eq!(
        c.store().join_snapshot(id).unwrap().state,
        JoinState::Collecting
    );
    let status = call(
        &mut c,
        "human",
        "status",
        json!({"op":"status","target":"caller"}),
    )
    .unwrap();
    assert_eq!(status["agent"]["open_joins"], json!([id]));
    DaemonReply::new(&status).unwrap().agent().unwrap();
}
#[test]
fn joined_batch_rejects_bad_options_modes_self_empty_and_request_id_collision() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    for id in ["caller", "a"] {
        c.store().save(&agent(id)).unwrap();
    }
    for options in [
        json!(null),
        json!(false),
        json!([]),
        json!([5]),
        json!({"deadline_seconds":0}),
        json!({"deadline_seconds":u64::MAX}),
        json!({"deadline_seconds":null}),
        json!({"unexpected":true}),
    ] {
        let mut req = request(json!([send("one", "a")]));
        req["join"] = options;
        assert!(call(&mut c, "caller", "round", req).is_err());
    }
    for mode in [json!("steer"), json!("redirect"), json!(null), json!(42)] {
        let mut operation = send("one", "a");
        operation["mode"] = mode;
        assert!(call(&mut c, "caller", "round", request(json!([operation]))).is_err());
    }
    for ops in [
        json!([send("one", "caller")]),
        json!([]),
        json!([{"op":"terminate","target":"a","request_id":"end"}]),
        json!([send("round", "a")]),
    ] {
        assert!(call(&mut c, "caller", "round", request(ops)).is_err());
        assert_eq!(
            c.store().agent("a").unwrap().task_outcome,
            TaskOutcome::Open
        );
    }
    assert!(c.store().open_joins("caller").unwrap().is_empty());
    for join in [json!({}), json!({"deadline_seconds":5})] {
        validate_tool(
            "hive_batch",
            &json!({"request_id":"r","join":join,"operations":[send("one","a")]}),
        )
        .unwrap();
    }
    for join in [json!([]), json!({"deadline_seconds":0}), json!({"extra":1})] {
        assert!(
            validate_tool(
                "hive_batch",
                &json!({"request_id":"r","join":join,"operations":[send("one","a")]})
            )
            .is_err()
        );
    }
}
#[test]
fn joined_batch_later_termination_settles_or_cancels_atomically() {
    for target in ["a", "caller"] {
        let temp = crate::runtime::test_directory();
        let mut c = core(temp.path());
        for id in ["caller", "a"] {
            c.store().save(&agent(id)).unwrap();
        }
        let accepted = call(
            &mut c,
            "caller",
            "round",
            request(json!([send("one","a"),{"op":"terminate","target":target,"request_id":"end"}])),
        )
        .unwrap();
        let snapshot = c
            .store()
            .join_snapshot(accepted["join"]["join_id"].as_str().unwrap())
            .unwrap();
        if target == "a" {
            assert_eq!(snapshot.state, JoinState::Completed);
            assert_eq!(snapshot.members[0].outcome, JoinOutcome::Undeliverable);
        } else {
            assert_eq!(snapshot.state, JoinState::Cancelled);
            assert_eq!(snapshot.members[0].outcome, JoinOutcome::Pending);
        }
    }
}
#[test]
fn joined_spawn_and_send_completion_keep_summary_without_duplicate_notification() {
    for spawn in [false, true] {
        let temp = crate::runtime::test_directory();
        let mut c = core(temp.path());
        configured(&mut c);
        c.store().save(&agent("caller")).unwrap();
        let operation = if spawn {
            json!({"op":"spawn","role":"test","cwd":temp.path(),"body":"work","request_id":"member","sandbox":"read-only"})
        } else {
            let mut child = agent("child");
            child.parent_id = Some("caller".into());
            c.store().save(&child).unwrap();
            send("member", "child")
        };
        let accepted = call(&mut c, "caller", "round", request(json!([operation]))).unwrap();
        let id = accepted["join"]["join_id"].as_str().unwrap().to_owned();
        let target = c.store().join_snapshot(&id).unwrap().members[0]
            .target
            .clone();
        c.atomic(|c| {
            let mut a = c.store().agent(&target)?;
            let m = c.store().next_message(&target)?.unwrap();
            a.execution_id = Some("execution".into());
            a.thread_id = Some("thread".into());
            a.turn_id = Some("turn".into());
            c.store().save(&a)?;
            c.store().message_state(&m, MessageState::Submitted)?;
            c.store().turn_started(
                &a,
                events::TurnStarted {
                    attachments: &[],
                    turn_id: "turn",
                    message_id: &m.id,
                    sender: &m.sender,
                    selection: events::TurnSelection {
                        model: &a.model,
                        effort: &a.effort,
                        options_revision: 0,
                        selection_verification: "requested_unverified",
                    },
                },
            )?;
            c.store().assistant_output(
                &a,
                Some(&m.id),
                json!({"turn_id":"turn","phase":"final_answer","text":"Done"}),
                true,
            )?;
            c.store().message_state(&m, MessageState::Completed)?;
            a.completion_pending = Some("Detailed completion summary".into());
            a.turn_id = None;
            c.finish_task_turn(&mut a, Some(&m.id))
        })
        .unwrap();
        let snapshot = c.store().join_snapshot(&id).unwrap();
        assert_eq!(snapshot.members[0].output.as_ref().unwrap().text, "Done");
        assert_eq!(
            snapshot.members[0].task_completed.as_ref().unwrap().text,
            "Detailed completion summary"
        );
        let messages = c.store().messages("caller", None, 200).unwrap();
        let incoming: Vec<_> = messages["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["recipient"] == "caller")
            .collect();
        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming[0]["kind"], "join_result");
    }
}
#[test]
fn joined_delayed_input_survives_requester_purge_and_gc() {
    for gc in [false, true] {
        let temp = crate::runtime::test_directory();
        let mut c = core(temp.path());
        for id in ["caller", "a"] {
            let mut a = agent(id);
            a.state = AgentState::Stopped;
            a.conversation_clean = true;
            c.store().save(&a).unwrap();
        }
        let mut operation = send("later", "a");
        operation["delay_seconds"] = json!(60);
        let accepted = call(&mut c, "caller", "round", request(json!([operation]))).unwrap();
        let deadline = accepted["results"][0]["not_before"].as_u64().unwrap();
        call(
            &mut c,
            "human",
            "end",
            json!({"op":"terminate","target":"caller"}),
        )
        .unwrap();
        let mut ended = c.store().agent("caller").unwrap();
        ended.control = None; // Simulate release of the never-launched process.
        c.store().save(&ended).unwrap();
        let removal = if gc {
            json!({"op":"gc","before":crate::state::now()+1})
        } else {
            json!({"op":"purge","target":"caller"})
        };
        call(&mut c, "human", "remove", removal).unwrap();
        assert!(c.store().agent("caller").is_err());
        assert!(
            c.store()
                .join_snapshot(accepted["join"]["join_id"].as_str().unwrap())
                .is_err()
        );
        c.atomic(|c| c.promote_scheduled(deadline)).unwrap();
        let message = c.store().next_message("a").unwrap().unwrap();
        assert!(!message.joined);
        assert_eq!(message.state, MessageState::Accepted);
    }
}

#[test]
fn idle_completion_after_joined_turn_keeps_ordinary_parent_notification() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    c.store().save(&agent("parent")).unwrap();
    let mut child = agent("child");
    child.parent_id = Some("parent".into());
    c.store().save(&child).unwrap();
    let accepted = call(
        &mut c,
        "parent",
        "round",
        request(json!([send("member", "child")])),
    )
    .unwrap();
    c.atomic(|c| {
        let input = c.store().next_message("child")?.unwrap();
        c.store().message_state(&input, MessageState::Completed)?;
        child.last_finished_turn = Some(crate::runtime::branching::FinishedTurn {
            thread: "thread".into(),
            execution: "execution".into(),
            turn: "turn".into(),
            message: Some(input.id),
            outcome: crate::runtime::domain::ProviderTurnStatus::Completed,
            at: crate::state::now(),
        });
        c.store().save(&child)
    })
    .unwrap();
    child.completion_pending = Some("Later idle completion".into());
    c.atomic(|c| c.finish_task(&mut child)).unwrap();
    let join = c
        .store()
        .join_snapshot(accepted["join"]["join_id"].as_str().unwrap())
        .unwrap();
    assert!(join.members[0].task_completed.is_none());
    assert!(
        c.store().messages("parent", None, 200).unwrap()["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["body"] == "Child task completed: Later idle completion")
    );
}

#[test]
fn join_input_absence_is_typed_only_at_read_boundary_after_option_validation() {
    let temp = crate::runtime::test_directory();
    let c = core(temp.path());
    let changes = c.store().total_changes();
    assert!(
        c.store()
            .find_join_snapshot("private-canary")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        c.store()
            .join_snapshot("private-canary")
            .unwrap_err()
            .code(),
        "invalid_request"
    );
    control_inputs::assert_input(
        pagination::observe(&c, json!({"op":"read","target":"join:private-canary"})),
        "invalid_value",
        "/target",
        "read",
    );
    for field in ["after", "limit"] {
        let mut request = json!({"op":"read","target":"join:private-canary"});
        request[field] = json!(0);
        control_inputs::assert_input(
            pagination::observe(&c, request),
            "conflicting_arguments",
            &format!("/{field}"),
            "read",
        );
    }
    assert_eq!(c.store().total_changes(), changes);
    let join = c.store().create_join("human", "round", None).unwrap();
    let found =
        pagination::observe(&c, json!({"op":"read","target":format!("join:{join}")})).unwrap();
    assert_eq!(found["join_id"], join);
}

fn join_open() -> Error {
    Error::join_open(
        "A join you requested is still collecting, or its result is unread. End your turn; the result arrives as new input. Completing now would cancel the join and end its with_parent workers. After a join deadline passes and its partial result is read, completion is allowed.",
    )
}
fn deliver(c: &mut Core, recipient: &str) {
    c.atomic(|c| {
        let message = c.store().next_message(recipient)?.unwrap();
        c.store().message_state(&message, MessageState::Completed)
    })
    .unwrap();
}
#[test]
fn agent_completion_waits_for_its_join_and_its_result() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    for id in ["caller", "a", "b", "other"] {
        c.store().save(&agent(id)).unwrap();
    }
    let complete = json!({"op":"complete","body":"done"});
    // Being a member of someone else's join does not hold an agent back.
    call(
        &mut c,
        "other",
        "theirs",
        request(json!([send("for-b", "b")])),
    )
    .unwrap();
    call(&mut c, "b", "b-done", complete.clone()).unwrap();
    assert!(c.store().agent("b").unwrap().completion_pending.is_some());

    let mut joined = request(json!([send("member", "a")]));
    joined["join"] = json!({"deadline_seconds":5});
    let accepted = call(&mut c, "caller", "round", joined).unwrap();
    let id = accepted["join"]["join_id"].as_str().unwrap().to_owned();
    let refused = |c: &mut Core| {
        assert_error(call(c, "caller", "done", complete.clone()), join_open());
        let caller = c.store().agent("caller").unwrap();
        assert!(caller.completion_pending.is_none());
        assert!(!c.store().request_used("caller", "done").unwrap());
    };
    refused(&mut c);
    // Once every member answers, the unread result still holds completion back.
    deliver(&mut c, "a");
    assert_eq!(
        c.store().join_snapshot(&id).unwrap().state,
        JoinState::Completed
    );
    refused(&mut c);
    deliver(&mut c, "caller");
    let done = call(&mut c, "caller", "done", complete).unwrap();
    assert_eq!(done["completion"], "pending_turn_boundary");
}
#[test]
fn after_a_join_deadline_the_requester_may_stop_waiting() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    for id in ["caller", "a"] {
        c.store().save(&agent(id)).unwrap();
    }
    let complete = json!({"op":"complete","body":"done"});
    let mut joined = request(json!([send("member", "a")]));
    joined["join"] = json!({"deadline_seconds":5});
    let accepted = call(&mut c, "caller", "round", joined).unwrap();
    let id = accepted["join"]["join_id"].as_str().unwrap().to_owned();
    // The deadline queues a partial result; members are still pending.
    c.atomic(|c| c.store().expire_join_deadlines(crate::state::now() + 10))
        .unwrap();
    assert_eq!(
        c.store().join_snapshot(&id).unwrap().state,
        JoinState::DeadlinePassed
    );
    assert_error(
        call(&mut c, "caller", "done", complete.clone()),
        join_open(),
    );
    deliver(&mut c, "caller");
    let done = call(&mut c, "caller", "done", complete).unwrap();
    assert_eq!(done["completion"], "pending_turn_boundary");
}
#[test]
fn humans_may_complete_an_agent_with_an_open_join() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    for id in ["caller", "a"] {
        c.store().save(&agent(id)).unwrap();
    }
    let accepted = call(
        &mut c,
        "caller",
        "round",
        request(json!([send("member", "a")])),
    )
    .unwrap();
    let id = accepted["join"]["join_id"].as_str().unwrap().to_owned();
    call(
        &mut c,
        "human",
        "done",
        json!({"op":"complete","target":"caller","body":"stop here"}),
    )
    .unwrap();
    c.atomic(|c| {
        let mut caller = c.store().agent("caller")?;
        c.finish_task(&mut caller)
    })
    .unwrap();
    assert_eq!(
        c.store().join_snapshot(&id).unwrap().state,
        JoinState::Cancelled
    );
}
#[test]
fn a_join_after_requesting_completion_is_refused() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    for id in ["caller", "a"] {
        c.store().save(&agent(id)).unwrap();
    }
    call(
        &mut c,
        "caller",
        "done",
        json!({"op":"complete","body":"done"}),
    )
    .unwrap();
    assert_error(
        call(
            &mut c,
            "caller",
            "round",
            request(json!([send("member", "a")])),
        ),
        Error::completion_pending(
            "Completion is pending the turn boundary; a join started now would be cancelled.",
        ),
    );
    assert!(c.store().open_joins("caller").unwrap().is_empty());
    assert!(!c.store().request_used("caller", "member").unwrap());
    // A batch without a join is unaffected.
    let plain = json!({"op":"batch","operations":[send("plain", "a")]});
    call(&mut c, "caller", "plain-round", plain).unwrap();
}
