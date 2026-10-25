use super::*;

pub(super) fn wait_completed_input(f: &Harness, id: &str, message: &str) {
    let mut page = Value::Null;
    let completed = support::wait::try_wait_until(
        support::wait::TEST_TIMEOUT,
        Duration::from_millis(30),
        || {
            page = f.mailbox(id);
            page["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["id"] == message && m["state"] == "completed")
                .then_some(())
        },
    );
    assert!(completed.is_some(), "{page}");
}

#[test]
fn agents_delegate_exchange_reviews_and_complete_atomically() {
    let mut f = Harness::new();
    f.start();
    let id = f.spawn("delegate");
    let parent = f.wait(&id, |a| {
        a["task_outcome"] == "completed" && a["state"] == "stopped"
    });
    assert!(!parent["thread_id"].is_null());
    let all = f.call(json!({"op":"agents"}));
    let agents = all["agents"].as_array().unwrap();
    assert_eq!(agents.len(), 2);
    let child = agents.iter().find(|a| a["parent_id"] == id).unwrap();
    assert_eq!(child["expired"], true);
    assert_eq!(child["model"], "gpt-5.6-sol");
    assert_ne!(child["thread_id"], parent["thread_id"]);
    let messages = f.mailbox(&id);
    assert!(
        messages["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["body"] == "NO_FINDINGS" && m["state"] == "completed")
    );
    let rejected = f.call(json!({"op":"send","target":id,"body":"again"}));
    assert_eq!(rejected["error"]["code"], "agent_completed");
    f.shutdown();
}
#[test]
fn retries_pause_resume_and_daemon_restart_preserve_conversation() {
    let mut f = Harness::new();
    f.start();
    let args = json!({"op":"spawn","role":"implementor","cwd":f.tmp.path(),"body":"idle"});
    let first = f.call_id("spawn-once", args.clone());
    assert_eq!(first, f.call_id("spawn-once", args));
    assert_eq!(
        f.call_id(
            "spawn-once",
            json!({"op":"spawn","role":"implementor","body":"different"})
        )["error"]["code"],
        "request_conflict"
    );
    let id = first["agent_id"].as_str().unwrap();
    wait_completed_input(&f, id, first["message_id"].as_str().unwrap());
    let original = f.wait(id, |a| a["state"] == "idle");
    f.ok(json!({"op":"stop","target":id}));
    f.wait(id, |a| a["state"] == "stopped");
    let queued = f.call(json!({"op":"send","target":id,"body":"idle"}));
    assert_eq!(
        f.call(json!({"op":"status","target":id}))["agent"]["paused"],
        true
    );
    f.ok(json!({"op":"resume","target":id}));
    wait_completed_input(&f, id, queued["message_id"].as_str().unwrap());
    let resumed = f.wait(id, |a| a["state"] == "idle");
    assert_eq!(original["thread_id"], resumed["thread_id"]);
    f.shutdown();
    f.start();
    f.ok(json!({"op":"send","target":id,"body":"complete"}));
    let done = f.wait(id, |a| a["task_outcome"] == "completed");
    assert_eq!(original["thread_id"], done["thread_id"]);
}
#[test]
fn malformed_tools_and_acknowledged_interrupt_do_not_fail_the_agent() {
    let mut f = Harness::new();
    f.start();
    let id = f.spawn("bad arguments");
    f.settled(&id);
    f.ok(json!({"op":"send","target":id,"body":"extra arguments"}));
    support::wait::wait_until(
        Duration::from_secs(5),
        support::wait::POLL_INTERVAL,
        "malformed-tool input completes",
        || {
            f.mailbox(&id)["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["body"] == "extra arguments" && m["state"] == "completed")
                .then_some(())
        },
    );
    assert_eq!(
        f.call(json!({"op":"status","target":id}))["agent"]["task_outcome"],
        "open"
    );
    f.ok(json!({"op":"send","target":id,"body":"hold"}));
    f.wait(&id, |a| a["state"] == "running");
    f.ok(json!({"op":"interrupt","target":id}));
    f.wait(&id, |a| a["state"] == "idle");
    std::thread::sleep(Duration::from_secs(11));
    assert_eq!(
        f.call(json!({"op":"status","target":id}))["agent"]["state"],
        "idle"
    );
}

#[test]
fn queued_messages_wake_after_restart_without_new_input() {
    let mut f = Harness::new();
    f.start();
    let id = f.spawn("hold");
    let before = f.wait(&id, |a| a["state"] == "running");
    f.ok(json!({"op":"send","target":id,"body":"complete"}));
    f.shutdown();
    f.start();
    let after = f.wait(&id, |a| a["task_outcome"] == "completed");
    assert_eq!(before["thread_id"], after["thread_id"]);
}

#[test]
fn terminal_cancellation_discards_queued_work_and_cannot_be_resumed_or_completed() {
    let mut f = Harness::new();
    f.start();
    let id = f.spawn("hold");
    f.wait(&id, |a| a["state"] == "running");
    let queued = f.call(json!({"op":"send","target":id,"body":"queued"}));
    f.ok(json!({"op":"complete","target":id,"body":"must not complete"}));
    let args = json!({"op":"terminate","target":id});
    let first = f.call_id("cancel-once", args.clone());
    assert_eq!(first["termination"], "requested");
    assert_eq!(first, f.call_id("cancel-once", args));
    let a = f.wait(&id, |a| a["state"] == "stopped");
    assert_eq!(a["task_outcome"], "cancelled");
    assert_eq!(a["expired"], false);
    assert!(a["completion_pending"].is_null());
    let messages = f.mailbox(&id);
    assert!(
        messages["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["id"] == queued["message_id"] && m["state"] == "undeliverable")
    );
    for op in ["resume", "terminate", "complete", "send"] {
        assert_eq!(
            f.call(json!({"op":op,"target":id,"body":"again"}))["error"]["code"],
            "agent_completed"
        );
    }
    let waiter = support::command(env!("CARGO_BIN_EXE_hivectl"))
        .arg("--state-dir")
        .arg(&f.root)
        .args(["wait", &id, "--timeout", "1"])
        .output()
        .unwrap();
    assert!(!waiter.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&waiter.stdout).unwrap()["agent"]["task_outcome"],
        "cancelled"
    );
    // Idle/paused queued-only cancellation never starts a new provider execution.
    let other = f.spawn("idle");
    let before = f.settled(&other);
    f.ok(json!({"op":"stop","target":other}));
    f.wait(&other, |a| a["state"] == "stopped");
    f.ok(json!({"op":"send","target":other,"body":"complete"}));
    f.ok(json!({"op":"terminate","target":other}));
    let cancelled = f.wait(&other, |a| a["control"].is_null());
    assert_eq!(cancelled["execution_id"], before["execution_id"]);
    assert_eq!(cancelled["task_outcome"], "cancelled");
    let completed = f.spawn("complete");
    f.wait(&completed, |a| a["state"] == "stopped");
    assert_eq!(
        f.call(json!({"op":"terminate","target":completed}))["error"]["code"],
        "agent_completed"
    );
    assert_eq!(
        f.call(json!({"op":"status","target":completed}))["agent"]["task_outcome"],
        "completed"
    );
    f.shutdown();
    assert!(f.reports().iter().all(|r| r["confirmed"] == true));
}

#[test]
fn cancelling_a_parent_expires_bound_children_and_preserves_independent_children() {
    let mut f = Harness::new();
    f.start();
    let parent = f.spawn("spawn mixed children");
    f.wait(&parent, |a| {
        a["state"] == "idle"
            && f.mailbox(&parent)["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["sender"] == "human" && m["state"] == "completed")
    });
    let all = f.call(json!({"op":"agents"}));
    let children: Vec<_> = all["agents"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["parent_id"] == parent)
        .collect();
    assert_eq!(children.len(), 2);
    for a in &children {
        f.wait(a["id"].as_str().unwrap(), |a| a["state"] == "idle");
    }
    f.ok(json!({"op":"terminate","target":parent}));
    f.wait(&parent, |a| a["state"] == "stopped");
    for child in children {
        let id = child["id"].as_str().unwrap();
        if child["lifetime"] == "with_parent" {
            let a = f.wait(id, |a| a["state"] == "stopped");
            assert_eq!(a["expired"], true);
            assert_eq!(
                f.call(json!({"op":"send","target":id,"body":"idle"}))["error"]["code"],
                "agent_completed"
            );
            continue;
        }
        let a = f.call(json!({"op":"status","target":id}))["agent"].clone();
        assert_eq!(a["task_outcome"], "open");
        assert_eq!(a["expired"], false);
        assert_eq!(a["state"], "idle");
        assert_eq!(
            f.call(json!({"op":"send","target":id,"body":"idle"}))["delivery_state"],
            "accepted"
        );
        f.ok(json!({"op":"stop","target":id}));
        f.wait(id, |a| a["state"] == "stopped");
        assert_eq!(
            f.call(json!({"op":"resume","target":id}))["status"],
            "requested"
        );
        f.wait(id, |a| a["state"] == "idle");
        f.ok(json!({"op":"terminate","target":id}));
        f.wait(id, |a| a["state"] == "stopped");
    }
    f.shutdown();
    assert!(f.reports().iter().all(|r| r["confirmed"] == true));
}
