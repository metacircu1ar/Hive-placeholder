use super::*;
use crate::runtime::domain::MessageState;

#[test]
fn delayed_acceptance_replays_the_original_deadline_and_never_wakes_recipient() {
    let tmp = crate::runtime::test_directory();
    let mut c = core(tmp.path());
    let mut a = agent("a");
    a.state = AgentState::Stopped;
    c.store().save(&a).unwrap();
    c.store().changes();
    let args = json!({"op":"send","target":"a","body":"future","delay_seconds":10});
    let first = call(&mut c, "human", "id", args.clone()).unwrap();
    assert_eq!(first["delivery_state"], "scheduled");
    assert!(first["not_before"].as_u64().unwrap() >= crate::state::now() + 10);
    let wake = c.store().changes();
    assert!(wake.deadlines);
    assert!(wake.agents.is_empty());
    assert!(!wake.all);
    assert_eq!(c.store().agent("a").unwrap().state, AgentState::Stopped);
    let id = first["message_id"].as_str().unwrap();
    assert_eq!(
        c.store().message_result(id).unwrap().delivery_state,
        MessageState::Scheduled
    );
    c.promote_scheduled(first["not_before"].as_u64().unwrap())
        .unwrap();
    assert_eq!(call(&mut c, "human", "id", args.clone()).unwrap(), first);
    a.task_outcome = TaskOutcome::Completed;
    c.store().save(&a).unwrap();
    c.store().undeliverable("a").unwrap();
    assert_eq!(call(&mut c, "human", "id", args).unwrap(), first);
    assert_eq!(
        call(
            &mut c,
            "human",
            "id",
            json!({"op":"send","target":"a","body":"future","delay_seconds":11})
        )
        .unwrap_err()
        .code(),
        "request_conflict"
    );
}
#[test]
fn delayed_validation_and_batch_rollback_are_strict() {
    let tmp = crate::runtime::test_directory();
    let mut c = core(tmp.path());
    c.store().save(&agent("a")).unwrap();
    for n in [
        json!(0),
        json!(604_801),
        json!(-1),
        json!(1.5),
        json!("2"),
        Value::Null,
    ] {
        assert_eq!(
            call(
                &mut c,
                "human",
                "bad",
                json!({"op":"send","target":"a","body":"future","delay_seconds":n})
            )
            .unwrap_err()
            .code(),
            "invalid_request"
        );
    }
    for mode in [
        json!("steer"),
        json!("redirect"),
        json!("unknown"),
        json!(0),
        Value::Null,
    ] {
        assert_eq!(
            call(
                &mut c,
                "human",
                "bad",
                json!({"op":"send","target":"a","body":"future","delay_seconds":1,"mode":mode})
            )
            .unwrap_err()
            .code(),
            "unsupported_operation"
        );
    }
    let batch = json!({"op":"batch","operations":[
        {"op":"send","target":"a","body":"future","delay_seconds":10,"request_id":"one"},
        {"op":"send","target":"missing","body":"bad","delay_seconds":10,"request_id":"two"}]});
    assert!(call(&mut c, "human", "batch", batch).is_err());
    assert!(
        c.store().messages("a", None, 200).unwrap()["messages"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(c.store().next_deadline().unwrap(), None);
    assert!(
        call(
            &mut c,
            "human",
            "one",
            json!({"op":"send","target":"a","body":"new","delay_seconds":604_800})
        )
        .is_ok()
    );
    assert!(
        validate_tool(
            "hive_send",
            &json!({"target":"a","body":"x","request_id":"r","delay_seconds":1})
        )
        .is_ok()
    );
}

#[test]
fn scheduled_children_expire_and_recipient_purge_removes_proof() {
    let tmp = crate::runtime::test_directory();
    let mut c = core(tmp.path());
    let mut parent = agent("parent");
    parent.state = AgentState::Stopped;
    c.store().save(&parent).unwrap();
    let mut child = agent("child");
    child.parent_id = Some("parent".into());
    child.lifetime = crate::config::Lifetime::WithParent;
    child.state = AgentState::Stopped;
    c.store().save(&child).unwrap();
    let reply = call(
        &mut c,
        "human",
        "timer",
        json!({"op":"send","target":"child","body":"future","delay_seconds":10}),
    )
    .unwrap();
    call(
        &mut c,
        "human",
        "end",
        json!({"op":"terminate","target":"parent"}),
    )
    .unwrap();
    assert!(c.store().agent("child").unwrap().expired);
    assert_eq!(
        c.store()
            .message_result(reply["message_id"].as_str().unwrap())
            .unwrap()
            .outcome,
        CallOutcome::Cancelled
    );
    let purged = call(
        &mut c,
        "human",
        "purge",
        json!({"op":"purge","target":"child"}),
    )
    .unwrap();
    assert_eq!(purged["counts"]["messages"], 1);
    assert_eq!(purged["counts"]["deliveries"], 1);
    assert_eq!(purged["counts"]["call_results"], 1);
    assert_eq!(c.store().next_deadline().unwrap(), None);
}

#[test]
fn scheduled_cancellation_quarantines_prior_delivery_evidence_before_the_deadline() {
    for ending in ["complete", "terminate", "expire"] {
        let tmp = crate::runtime::test_directory();
        let mut c = core(tmp.path());
        let mut parent = agent("parent");
        parent.state = AgentState::Stopped;
        c.store().save(&parent).unwrap();
        let mut a = agent("a");
        a.state = AgentState::Stopped;
        a.parent_id = Some("parent".into());
        a.lifetime = crate::config::Lifetime::WithParent;
        c.store().save(&a).unwrap();
        let request = json!({"op":"send","target":"a","body":"future","delay_seconds":604_800});
        let bad = call(&mut c, "human", "bad", request.clone()).unwrap();
        let good = call(&mut c, "human", "good", request).unwrap();
        let bad = bad["message_id"].as_str().unwrap();
        let good = good["message_id"].as_str().unwrap();
        let db = rusqlite::Connection::open(tmp.path().join("runtime.sqlite")).unwrap();
        db.execute("UPDATE deliveries SET status='submitted',execution='conflicting',recipient_turn='old-turn',error='original' WHERE message_id=?1",[bad]).unwrap();
        db.execute(
            "UPDATE call_results SET start_seq=7,output='{}',output_final=1 WHERE message_id=?1",
            [bad],
        )
        .unwrap();
        assert_eq!(
            c.store().message_result(bad).unwrap_err().code(),
            "database_error"
        );
        match ending {
            "complete" => {
                a.completion_pending = Some("done".into());
                c.atomic(|c| c.finish_task(&mut a)).unwrap();
            }
            "terminate" => {
                call(
                    &mut c,
                    "human",
                    "end",
                    json!({"op":"terminate","target":"a"}),
                )
                .unwrap();
            }
            _ => {
                call(
                    &mut c,
                    "human",
                    "end",
                    json!({"op":"terminate","target":"parent"}),
                )
                .unwrap();
            }
        }
        assert_eq!(
            c.store().message_result(bad).unwrap_err().code(),
            "database_error"
        );
        assert_eq!(
            c.store().message_result(good).unwrap().outcome,
            CallOutcome::Cancelled
        );
        let proof:i64=db.query_row("SELECT count(*) FROM deliveries d JOIN call_results c ON c.message_id=d.message_id WHERE d.message_id=?1 AND d.status='submitted' AND d.execution='conflicting' AND d.recipient_turn='old-turn' AND d.error='original' AND c.start_seq=7 AND c.output='{}' AND c.output_final=1",[bad],|r|r.get(0)).unwrap();
        assert_eq!(proof, 1);
        assert!(
            db.execute("UPDATE messages SET state='accepted' WHERE id=?1", [bad])
                .is_err()
        );
        assert_eq!(c.store().next_deadline().unwrap(), None);
        assert_eq!(
            c.promote_scheduled(u64::try_from(i64::MAX).unwrap())
                .unwrap(),
            0
        );
    }
}

#[test]
fn completed_scheduled_input_is_removed_by_gc_with_its_retained_call() {
    let tmp = crate::runtime::test_directory();
    let mut c = core(tmp.path());
    let mut a = agent("a");
    a.state = AgentState::Stopped;
    c.store().save(&a).unwrap();
    let accepted = call(
        &mut c,
        "human",
        "timer",
        json!({"op":"send","target":"a","body":"future","delay_seconds":100}),
    )
    .unwrap();
    a.completion_pending = Some("done".into());
    c.atomic(|c| c.finish_task(&mut a)).unwrap();
    let id = accepted["message_id"].as_str().unwrap();
    assert_eq!(
        c.store().message_result(id).unwrap().outcome,
        CallOutcome::Cancelled
    );
    let gc = call(
        &mut c,
        "human",
        "gc",
        json!({"op":"gc","before":crate::state::now()+1}),
    )
    .unwrap();
    assert_eq!(gc["stats"]["messages"], 1, "{gc}");
    assert_eq!(
        c.store().message_result(id).unwrap_err().code(),
        "unknown_message"
    );
    assert_eq!(c.store().next_deadline().unwrap(), None);
}
