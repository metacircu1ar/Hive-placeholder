use super::*;

fn family(c: &Core) {
    c.store().save(&agent("coordinator")).unwrap();
    let mut child = agent("worker");
    child.parent_id = Some("coordinator".into());
    child.lifetime = crate::config::Lifetime::Independent;
    c.store().save(&child).unwrap();
}
fn send(target: &str) -> Value {
    json!({"op":"send","target":target,"body":"hello"})
}

#[test]
fn relative_send_and_terminate_store_canonical_ids_and_replay_before_resolution() {
    for (caller, target, recipient, id) in [
        ("coordinator", "@child", "worker", "ping"),
        ("worker", "@parent", "coordinator", "pong"),
    ] {
        let tmp = crate::runtime::test_directory();
        let mut c = core(tmp.path());
        family(&c);
        let reply = call(&mut c, caller, id, send(target)).unwrap();
        assert_eq!(reply["recipient"], recipient);
        let message = c.store().next_message(recipient).unwrap().unwrap();
        assert_eq!(message.id, reply["message_id"]);
        assert_eq!(message.sender, caller);
        assert_eq!(message.recipient, recipient);
        let request = json!({"op":"terminate","target":"@child"});
        let ended = call(&mut c, "coordinator", "end-child", request.clone()).unwrap();
        assert_eq!(ended["agent_id"], "worker");
        assert!(c.store().agent("worker").unwrap().terminal());
        // Fresh requests apply the ended child's ordinary lifecycle checks.
        assert_eq!(
            call(&mut c, "coordinator", "end-again", request.clone())
                .unwrap_err()
                .code(),
            "agent_completed"
        );
        // New siblings must neither retarget a retry nor duplicate its message.
        let mut sibling = agent("sibling");
        sibling.parent_id = Some("coordinator".into());
        c.store().save(&sibling).unwrap();
        assert_eq!(call(&mut c, caller, id, send(target)).unwrap(), reply);
        assert_eq!(
            call(&mut c, "coordinator", "end-child", request).unwrap(),
            ended
        );
    }
}

#[test]
fn relative_targets_reject_external_missing_and_ambiguous_relations() {
    let tmp = crate::runtime::test_directory();
    let mut c = core(tmp.path());
    c.store().save(&agent("coordinator")).unwrap();
    for (caller, target) in [
        ("human", "@parent"),
        ("human", "@child"),
        ("coordinator", "@parent"),
        ("coordinator", "@child"),
    ] {
        for request in [send(target), json!({"op":"terminate","target":target})] {
            let error = call(&mut c, caller, "absent", request).unwrap_err();
            assert_eq!(error.code(), "invalid_value");
            assert!(serde_json::to_string(&error).unwrap().contains("/target"));
        }
    }
    family(&c);
    let mut second = agent("second");
    second.parent_id = Some("coordinator".into());
    c.store().save(&second).unwrap();
    for request in [send("@child"), json!({"op":"terminate","target":"@child"})] {
        let error = call(&mut c, "coordinator", "ambiguous", request).unwrap_err();
        assert_eq!(error.code(), "invalid_value");
        assert!(
            serde_json::to_string(&error)
                .unwrap()
                .contains("hive_agents")
        );
    }
    // Explicit IDs remain available when there are multiple children.
    call(&mut c, "coordinator", "explicit", send("worker")).unwrap();
    assert_eq!(
        call(&mut c, "coordinator", "typo", send("wroker"))
            .unwrap_err()
            .code(),
        "unknown_agent"
    );
}

#[test]
fn ended_children_still_make_child_alias_ambiguous_and_grandchildren_do_not() {
    for (index, outcome, expired) in [
        (0, TaskOutcome::Completed, false),
        (1, TaskOutcome::Cancelled, false),
        (2, TaskOutcome::Open, true),
    ] {
        let tmp = crate::runtime::test_directory();
        let mut c = core(tmp.path());
        family(&c);
        let mut grandchild = agent("grandchild");
        grandchild.parent_id = Some("worker".into());
        c.store().save(&grandchild).unwrap();
        assert_eq!(
            call(&mut c, "coordinator", "direct", send("@child")).unwrap()["recipient"],
            "worker"
        );
        let mut old = agent(&format!("old-{index}"));
        old.parent_id = Some("coordinator".into());
        old.task_outcome = outcome;
        old.expired = expired;
        c.store().save(&old).unwrap();
        assert_eq!(
            call(&mut c, "coordinator", "ambiguous", send("@child"))
                .unwrap_err()
                .code(),
            "invalid_value"
        );
    }
}

#[test]
fn relative_batch_join_is_canonical_atomic_and_replayable() {
    let tmp = crate::runtime::test_directory();
    let mut c = core(tmp.path());
    family(&c);
    let request = json!({"op":"batch","join":{},"operations":[
        {"op":"send","target":"@child","body":"review","request_id":"first"},
        {"op":"send","target":"missing","body":"review","request_id":"second"}
    ]});
    assert_eq!(
        call(&mut c, "coordinator", "round", request.clone())
            .unwrap_err()
            .code(),
        "unknown_agent"
    );
    assert!(c.store().next_message("worker").unwrap().is_none());
    assert!(!c.store().request_used("coordinator", "first").unwrap());
    let mut request = request;
    request["operations"].as_array_mut().unwrap().pop();
    let accepted = call(&mut c, "coordinator", "round", request.clone()).unwrap();
    assert_eq!(accepted["results"][0]["recipient"], "worker");
    let mut sibling = agent("sibling");
    sibling.parent_id = Some("coordinator".into());
    c.store().save(&sibling).unwrap();
    assert_eq!(
        call(&mut c, "coordinator", "round", request).unwrap(),
        accepted
    );
}

#[test]
fn parent_alias_survives_ended_parent_and_rejects_purged_parent_normally() {
    let tmp = crate::runtime::test_directory();
    let mut c = core(tmp.path());
    family(&c);
    let request = json!({"op":"terminate","target":"@parent"});
    let ended = call(&mut c, "worker", "end-parent", request.clone()).unwrap();
    assert_eq!(ended["agent_id"], "coordinator");
    assert_eq!(
        call(&mut c, "worker", "end-parent", request).unwrap(),
        ended
    );
    assert_eq!(
        call(&mut c, "worker", "send-ended", send("@parent"))
            .unwrap_err()
            .code(),
        "agent_completed"
    );
    let mut parent = c.store().agent("coordinator").unwrap();
    parent.state = AgentState::Stopped;
    parent.control = None;
    c.store().save(&parent).unwrap();
    call(
        &mut c,
        "human",
        "purge",
        json!({"op":"purge","target":"coordinator"}),
    )
    .unwrap();
    assert_eq!(
        call(&mut c, "worker", "send-purged", send("@parent"))
            .unwrap_err()
            .code(),
        "agent_purged"
    );
}
