use super::*;
use support::wait::{POLL_INTERVAL, TEST_TIMEOUT, wait_until};

/// Wait until every accepted message has run: the agent is idle with nothing
/// queued, and its newest message or turn event is a finished turn. State alone
/// passes too early: a fresh spawn is idle while its body is queued, and a claimed
/// message has left the queue before its turn starts.
fn idle(f: &Harness, id: &str) {
    wait_until(
        TEST_TIMEOUT,
        POLL_INTERVAL,
        "idle agent after its last turn",
        || {
            let row = f.ok(json!({"op":"tree","root":id}))["agents"][0].clone();
            let idle = row["state"] == "idle" && row["queued"] == 0;
            let read = f.read(id);
            let last = read["events"]
                .as_array()
                .unwrap()
                .iter()
                .rev()
                .find(|event| {
                    ["message_accepted", "turn_started", "turn_finished"]
                        .contains(&event["type"].as_str().unwrap_or_default())
                })
                .map(|event| event["type"].clone());
            (idle && last == Some(json!("turn_finished")) && f.status(id)["turn_id"].is_null())
                .then_some(())
        },
    );
}

/// Poll `live` until a turn completes, returning every item and whether a
/// durable change was signalled.
fn until_turn_completed(f: &Harness, id: &str, mut cursor: Value) -> (Vec<Value>, bool) {
    let mut items = Vec::new();
    let mut durable = false;
    wait_until(TEST_TIMEOUT, POLL_INTERVAL, "live turn completion", || {
        let page = f.ok(json!({"op":"live","target":id,"after":cursor,"wait_ms":2000}));
        assert_eq!(page["resync"], false, "{page}");
        cursor = page["cursor"].clone();
        durable |= page["durable_changed"] == true;
        items.extend(page["items"].as_array().unwrap().iter().cloned());
        items
            .iter()
            .any(|item| item["kind"] == "turn_completed")
            .then_some(())
    });
    (items, durable)
}

fn text_of(items: &[Value], kind: &str) -> (String, Value) {
    let matching: Vec<_> = items.iter().filter(|item| item["kind"] == kind).collect();
    assert!(!matching.is_empty(), "no {kind} in {items:?}");
    let item_id = matching[0]["item_id"].clone();
    assert!(
        matching.iter().all(|item| item["item_id"] == item_id),
        "{matching:?}"
    );
    (
        matching
            .iter()
            .map(|item| item["text"].as_str().unwrap())
            .collect(),
        item_id,
    )
}

/// The durable message text with this item key. The writer commits it after
/// the live item was published, so it is waited for.
fn durable_text(f: &Harness, id: &str, item_id: &Value) -> String {
    let message = format!("durable message keyed {item_id}");
    wait_until(TEST_TIMEOUT, POLL_INTERVAL, &message, || {
        f.read(id)["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["type"] == "assistant_message")
            .find(|event| event["data"]["item_id"] == *item_id)
            .map(|event| event["data"]["text"].as_str().unwrap().to_owned())
    })
}

#[test]
fn codex_live_items_stream_between_turn_markers_and_match_durable_messages() {
    let mut f = Harness::builder(Provider::Codex)
        .scenario(json!({"live_deltas":true,"record_initialize":true}))
        .build();
    f.start();
    let id = f.spawn("reply: first");
    idle(&f, &id);
    let attached = f.ok(json!({"op":"attach","target":id}));
    assert_eq!(attached["agent"]["id"], id);
    assert!(!attached["events"]["events"].as_array().unwrap().is_empty());
    assert_eq!(
        (attached["queued"].clone(), attached["mail"].clone()),
        (json!(0), json!(0))
    );
    let answer = "streamed answer";
    f.ok(json!({"op":"send","target":id,"body":format!("reply: {answer}")}));
    let (items, durable) = until_turn_completed(&f, &id, attached["live"].clone());
    let kinds: Vec<_> = items
        .iter()
        .map(|item| item["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds.first(), Some(&"turn_started"), "{kinds:?}");
    assert_eq!(kinds.last(), Some(&"turn_completed"), "{kinds:?}");
    let started = items.iter().find(|i| i["kind"] == "tool_started").unwrap();
    assert_eq!(
        (&started["tool"], &started["text"]),
        (&json!("command"), &json!("echo live"))
    );
    let done = items
        .iter()
        .find(|i| i["kind"] == "tool_completed")
        .unwrap();
    assert_eq!(
        (&done["status"], &done["text"]),
        (&json!("completed"), &json!("exit 0"))
    );
    assert_eq!(text_of(&items, "tool_output").0, "live\n");
    assert_eq!(text_of(&items, "reasoning").0, "Planning the answer.");
    let (text, item_id) = text_of(&items, "text");
    assert!(text.ends_with(answer), "{text}");
    // User-message echoes and raw reasoning never become live items.
    assert!(
        !items
            .iter()
            .any(|i| i.to_string().contains("echoed input") || i["text"] == "raw")
    );
    // The durable message carries the same key, so a viewer can replace streamed text.
    assert_eq!(durable_text(&f, &id, &item_id), text);
    assert!(durable, "durable changes were not signalled");
    // Session processes opt in to displayed deltas only; discovery keeps all off.
    let all = json!([
        "item/agentMessage/delta",
        "item/reasoning/textDelta",
        "item/reasoning/summaryTextDelta",
        "item/commandExecution/outputDelta"
    ]);
    let raw = json!(["item/reasoning/textDelta"]);
    let opt_outs: Vec<Value> = fs::read_to_string(f.provider_home.join("initialize.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap()["capabilities"]["optOutNotificationMethods"].clone())
        .collect();
    assert!(opt_outs.contains(&raw), "{opt_outs:?}");
    assert!(
        opt_outs.iter().all(|value| *value == raw || *value == all),
        "{opt_outs:?}"
    );
    f.shutdown();
}

#[test]
fn claude_partial_text_streams_and_matches_its_native_message() {
    let mut f = claude::fixture();
    f.set_scenario(Provider::Claude, json!({"live_deltas":true}));
    f.start();
    let id = claude::spawn(&f, "reply: first");
    idle(&f, &id);
    let attached = f.ok(json!({"op":"attach","target":id}));
    let answer = "partial answer";
    f.ok(json!({"op":"send","target":id,"body":format!("reply: {answer}")}));
    let (items, _) = until_turn_completed(&f, &id, attached["live"].clone());
    let (text, item_id) = text_of(&items, "text");
    assert!(
        text.ends_with(answer),
        "{text}\nATTACHED {attached}\nITEMS {items:?}\nEVENTS {}",
        f.read(&id)
    );
    assert_eq!(text_of(&items, "reasoning").0, "Planning.");
    assert!(item_id.as_str().unwrap().starts_with("msg_"));
    assert_eq!(durable_text(&f, &id, &item_id), text);
    f.shutdown();
}

#[test]
fn tree_long_polls_wake_on_spawn_and_restarts_resync_live_viewers() {
    let mut f = Harness::builder(Provider::Codex).build();
    f.start();
    let first = f.spawn("reply: one");
    idle(&f, &first);
    let tree = f.ok(json!({"op":"tree"}));
    let ids = |tree: &Value| -> Vec<Value> {
        tree["agents"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["id"].clone())
            .collect()
    };
    assert_eq!(ids(&tree), [json!(first)]);
    let row = &tree["agents"][0];
    assert_eq!(
        (&row["state"], &row["queued"], &row["children"]),
        (&json!("idle"), &json!(0), &json!(0))
    );
    assert_eq!(tree["truncated"], false);
    let second = f.spawn("reply: two");
    // The revision already moved, so this poll returns without waiting.
    let started = Instant::now();
    let next = f.ok(json!({"op":"tree","after":tree["version"],"wait_ms":20000}));
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(next["version"]["revision"].as_u64() > tree["version"]["revision"].as_u64());
    assert!(ids(&next).contains(&json!(second)));
    let subtree = f.ok(json!({"op":"tree","root":first}));
    assert_eq!(ids(&subtree), [json!(first)]);
    // Every live agent, without waiting; `all` excludes `root`.
    let all = f.ok(json!({"op":"tree","all":true}));
    assert_eq!(ids(&all), [json!(first), json!(second)]);
    let conflict = f.expect_err(
        json!({"op":"tree","all":true,"root":first}),
        "conflicting_arguments",
    );
    assert_eq!(conflict["error"]["field"], "/all");
    // Live without attach and invalid inputs are refused or resynced, not opened.
    let unknown_cursor = json!({"epoch":1,"generation":1,"seq":0,"durable":0});
    let page = f.ok(json!({"op":"live","target":second,"after":unknown_cursor,"wait_ms":0}));
    assert_eq!(
        (&page["resync"], &page["cursor"]),
        (&json!(true), &Value::Null)
    );
    f.expect_err(
        json!({"op":"live","target":first,"after":unknown_cursor,"wait_ms":25001}),
        "invalid_value",
    );
    f.expect_err(json!({"op":"attach","target":"missing"}), "unknown_agent");
    // Batches accept only spawn, send and terminate, so viewers never reach the writer.
    let batch = f.call(json!({"op":"batch","operations":[{"op":"tree","request_id":"t"}]}));
    assert_eq!(batch["error"]["field"], "/operations/0/op", "{batch}");
    let attached = f.ok(json!({"op":"attach","target":first}));
    f.shutdown();
    f.start();
    let page = f.ok(json!({"op":"live","target":first,"after":attached["live"],"wait_ms":0}));
    assert_eq!(
        (&page["resync"], &page["cursor"]),
        (&json!(true), &Value::Null)
    );
    f.shutdown();
}

#[test]
fn waiting_viewers_never_hold_turns_and_shutdown_ends_their_polls() {
    let mut f = Harness::builder(Provider::Codex)
        .scenario(json!({"live_deltas":true,"delta_delay":0.01}))
        .build();
    f.start();
    let id = f.spawn("reply: first");
    idle(&f, &id);
    // An attached viewer that never polls cannot slow its agent's turns.
    f.ok(json!({"op":"attach","target":id}));
    for body in ["reply: a", "reply: b", "reply: c"] {
        f.ok(json!({"op":"send","target":id,"body":body}));
    }
    wait_until(TEST_TIMEOUT, POLL_INTERVAL, "three viewed turns", || {
        let read = f.read(&id);
        (read["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["type"] == "assistant_message")
            .count()
            >= 4)
            .then_some(())
    });
    let attached = f.ok(json!({"op":"attach","target":id}));
    let root = f.root.clone();
    let target = id.clone();
    // Follow like a viewer: shutdown's own events may answer one poll first, but
    // no poll waits out its 25 s and the loop ends with a stopping daemon.
    let poll = std::thread::spawn(move || {
        let started = Instant::now();
        let mut after = attached["live"].clone();
        loop {
            let polled = Instant::now();
            let reply = runtime::client(
                &root,
                &Uuid::new_v4().to_string(),
                json!({"op":"live","target":target,"after":after,"wait_ms":25000}),
            );
            let code = match &reply {
                Ok(value) => value["error"]["code"].as_str().map(str::to_owned),
                Err(error) => Some(error.code().to_owned()),
            };
            assert!(polled.elapsed() < Duration::from_secs(15), "{reply:?}");
            if let Some(code) = code {
                return (code, started.elapsed());
            }
            after = reply.unwrap()["cursor"].clone();
        }
    });
    std::thread::sleep(Duration::from_millis(300));
    let stopping = Instant::now();
    f.shutdown();
    assert!(stopping.elapsed() < Duration::from_secs(10));
    let (code, waited) = poll.join().unwrap();
    assert!(
        code == "daemon_stopping" || code == "daemon_unavailable",
        "{code}"
    );
    assert!(waited < Duration::from_secs(15), "{waited:?}");
}

/// Start a long poll that must be answered by a change, not its timeout.
fn background_poll(f: &Harness, request: Value) -> std::thread::JoinHandle<Value> {
    let root = f.root.clone();
    std::thread::spawn(move || {
        let started = Instant::now();
        let reply = runtime::client(&root, &Uuid::new_v4().to_string(), request).unwrap();
        assert!(started.elapsed() < Duration::from_secs(10), "{reply}");
        reply
    })
}

/// A tree version and live cursor taken once the agent's history is quiet.
fn quiet_versions(f: &Harness, id: &str) -> (Value, Value) {
    wait_until(TEST_TIMEOUT, POLL_INTERVAL, "quiet viewer cursors", || {
        let version = f.ok(json!({"op":"tree"}))["version"].clone();
        let live = f.ok(json!({"op":"attach","target":id}))["live"].clone();
        let tree = f.ok(json!({"op":"tree","after":version,"wait_ms":400}));
        let page = f.ok(json!({"op":"live","target":id,"after":live,"wait_ms":400}));
        (tree["version"] == version && page["cursor"] == live).then_some((version, live))
    })
}

#[test]
fn approvals_and_termination_wake_waiting_tree_and_live_polls() {
    let mut f = Harness::builder(Provider::Codex).build();
    f.start();
    let id = f.spawn("reply: ready");
    idle(&f, &id);
    let ask = json!({"request_id":"r","message":"Deploy now?"});
    f.ok(json!({"op":"send","target":id,"body":format!("ask:{ask}")}));
    let approval = f.wait(&id, |a| a["waiting_on"] == "human")["approval_id"].clone();
    let rows = |tree: &Value| tree["agents"].as_array().unwrap().clone();
    let row = |tree: &Value| rows(tree).into_iter().find(|row| row["id"] == id);
    assert_eq!(
        row(&f.ok(json!({"op":"tree"}))).unwrap()["approval_id"],
        approval
    );
    // Closing the approval wakes both viewers, and the row no longer shows it.
    let (version, live) = quiet_versions(&f, &id);
    let tree = background_poll(&f, json!({"op":"tree","after":version,"wait_ms":25000}));
    let page = background_poll(
        &f,
        json!({"op":"live","target":id,"after":live,"wait_ms":25000}),
    );
    std::thread::sleep(Duration::from_millis(300));
    f.ok(json!({"op":"respond","approval_id":approval,"action":"cancel"}));
    assert_ne!(tree.join().unwrap()["version"], version);
    assert_eq!(page.join().unwrap()["durable_changed"], true);
    wait_until(TEST_TIMEOUT, POLL_INTERVAL, "closed approval row", || {
        row(&f.ok(json!({"op":"tree"})))
            .is_some_and(|row| row["approval_id"].is_null())
            .then_some(())
    });
    // Termination removes the root from the tree and wakes its viewer.
    idle(&f, &id);
    let (version, live) = quiet_versions(&f, &id);
    let tree = background_poll(&f, json!({"op":"tree","after":version,"wait_ms":25000}));
    let page = background_poll(
        &f,
        json!({"op":"live","target":id,"after":live,"wait_ms":25000}),
    );
    std::thread::sleep(Duration::from_millis(300));
    f.ok(json!({"op":"terminate","target":id}));
    assert_ne!(tree.join().unwrap()["version"], version);
    assert_eq!(page.join().unwrap()["durable_changed"], true);
    wait_until(
        TEST_TIMEOUT,
        POLL_INTERVAL,
        "ended root leaves the tree",
        || row(&f.ok(json!({"op":"tree"}))).is_none().then_some(()),
    );
    f.shutdown();
}

#[test]
fn attach_pages_forward_and_backward_over_durable_history() {
    let mut f = Harness::builder(Provider::Codex).build();
    f.start();
    let id = f.spawn("reply: one");
    idle(&f, &id);
    f.ok(json!({"op":"send","target":id,"body":"reply: two"}));
    idle(&f, &id);
    let all: Vec<Value> = f.read(&id)["events"].as_array().unwrap().clone();
    let seq = |event: &Value| event["seq"].as_u64().unwrap();
    assert!(all.len() > 4, "{all:?}");
    let newest = f.ok(json!({"op":"attach","target":id,"limit":2}));
    assert_eq!(
        newest["events"]["events"].as_array().unwrap(),
        &all[all.len() - 2..]
    );
    // Page backwards from the newest page until no older history remains.
    let mut collected = newest["events"]["events"].as_array().unwrap().clone();
    let mut before = seq(&collected[0]);
    loop {
        let page = f.ok(json!({"op":"attach","target":id,"before":before,"limit":3}));
        let events = page["events"]["events"].as_array().unwrap().clone();
        assert!(events.len() <= 3);
        assert_eq!(page["events"]["history_expired"], false);
        collected.splice(0..0, events);
        match page["events"]["older"].as_u64() {
            Some(older) => before = older,
            None => break,
        }
    }
    assert_eq!(collected, all);
    // The forward page after a durable cursor matches `read`.
    let after = f.ok(json!({"op":"attach","target":id,"after":seq(&all[1])}));
    assert_eq!(after["events"]["events"].as_array().unwrap(), &all[2..]);
    assert_eq!(
        after["events"]["cursor"].as_u64(),
        Some(seq(&all[all.len() - 1]))
    );
    for (args, code, field) in [
        (
            json!({"after":1,"before":5}),
            "conflicting_arguments",
            "/before",
        ),
        (json!({"before":0}), "out_of_range", "/before"),
        (json!({"after":"x"}), "invalid_value", "/after"),
        (json!({"limit":0}), "out_of_range", "/limit"),
    ] {
        let mut request = json!({"op":"attach","target":id});
        request
            .as_object_mut()
            .unwrap()
            .extend(args.as_object().unwrap().clone());
        let error = f.expect_err(request, code);
        assert_eq!(error["error"]["field"], field, "{error}");
    }
    f.shutdown();
}
