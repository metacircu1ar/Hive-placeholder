use super::*;
use std::sync::mpsc::Receiver;

#[test]
fn retries_reuse_one_request_id_and_stop_at_the_limit() {
    let mut ids = Vec::new();
    let mut waits = Vec::new();
    let mut outcomes = vec![
        Outcome::Transport,
        Outcome::Transport,
        Outcome::Ok(json!({"ok": 1})),
    ]
    .into_iter();
    let result = retry(
        "action-1",
        |id| {
            ids.push(id.to_owned());
            outcomes.next().unwrap()
        },
        |attempt| waits.push(attempt),
    );
    assert_eq!(result, Ok(json!({"ok": 1})));
    assert_eq!(ids, ["action-1"; 3]);
    assert_eq!(waits, [0, 1]);
    // A daemon that never answers is given up after the limit.
    let mut calls = 0;
    let result = retry(
        "action-2",
        |_| {
            calls += 1;
            Outcome::Transport
        },
        |_| {},
    );
    assert!(result.unwrap_err().starts_with("daemon_unavailable"));
    assert_eq!(calls, RETRY_LIMIT);
    // A refusal is final: the same write is not sent again.
    let mut calls = 0;
    let result = retry(
        "action-3",
        |_| {
            calls += 1;
            Outcome::Failed {
                code: ErrorCode::Input(hive_input::Code::InvalidValue),
                message: String::from("Bad body."),
                reply: Value::Null,
            }
        },
        |_| {},
    );
    assert_eq!(
        (result, calls),
        (Err(String::from("invalid_value: Bad body.")), 1)
    );
}

#[test]
fn backoff_grows_and_is_capped() {
    assert_eq!(backoff(0), Duration::from_millis(250));
    assert_eq!(backoff(2), Duration::from_secs(1));
    assert_eq!(backoff(10), MAX_BACKOFF);
}

fn capacity() -> Outcome {
    Outcome::Failed {
        code: ErrorCode::WatchCapacity,
        message: String::from("Too many viewers are waiting; retry after a short delay."),
        reply: Value::Null,
    }
}

/// A follower whose reads answer from `script`, then report the agent purged.
fn scripted(
    script: Vec<Outcome>,
) -> (
    Follower,
    Receiver<Update>,
    std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
) {
    let (updates, received) = std::sync::mpsc::sync_channel(UPDATE_SLOTS);
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = requests.clone();
    let mut script = script.into_iter();
    let reader: Reader = Box::new(move |request: &Value| {
        log.lock().unwrap().push(request.clone());
        script.next().unwrap_or(Outcome::Failed {
            code: ErrorCode::UnknownAgent,
            message: String::new(),
            reply: Value::Null,
        })
    });
    (
        Follower::new(String::from("agent"), updates, reader, |_| {}),
        received,
        requests,
    )
}

fn attached() -> Outcome {
    Outcome::Ok(
        json!({"agent": {"id": "agent", "approval_id": null}, "children": 0,
        "live": {"epoch": 1, "generation": 1, "seq": 0, "durable": 0},
        "events": {"events": [], "cursor": 0}}),
    )
}

#[test]
fn capacity_refusals_show_as_waiting_and_are_retried() {
    let page = Outcome::Ok(
        json!({"cursor": {"epoch": 1, "generation": 1, "seq": 1, "durable": 0},
        "items": [], "resync": false, "durable_changed": false}),
    );
    let (follower, updates, requests) = scripted(vec![
        capacity(),
        capacity(),
        attached(),
        attached(),
        capacity(),
        page,
    ]);
    follower.run();
    let kinds: Vec<String> = updates
        .try_iter()
        .map(|update| match update {
            Update::Waiting(waiting) => format!("waiting {waiting}"),
            Update::Snapshot(_) => String::from("snapshot"),
            Update::Refresh(_) => String::from("refresh"),
            Update::Purged => String::from("purged"),
            other => format!("{other:?}"),
        })
        .collect();
    // The first attach waits for a channel instead of ending the view; a live
    // wait reports the status once and clears it after the next page.
    assert_eq!(
        kinds,
        [
            "waiting true",
            "waiting false",
            "snapshot",
            "refresh",
            "waiting true",
            "waiting false",
            "purged"
        ]
    );
    let waits: Vec<Value> = requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request["op"] == "live")
        .map(|request| request["wait_ms"].clone())
        .collect();
    // After a refusal the wait is short, so the status clears soon.
    assert_eq!(
        waits,
        [
            json!(LONG_POLL_MS),
            json!(SLOT_PROBE_MS),
            json!(LONG_POLL_MS)
        ]
    );
}

#[test]
fn a_stopping_daemon_is_waited_for_like_an_unreachable_one() {
    let stopping = json!({"error": {"code": "daemon_stopping",
        "message": "The daemon is stopping; reconnect after it restarts."}});
    assert!(matches!(classify(stopping), Outcome::Transport));
    let refused = json!({"error": {"code": "unknown_agent", "message": "No such agent."}});
    assert!(
        matches!(classify(refused), Outcome::Failed { code, .. } if code == ErrorCode::UnknownAgent)
    );
    assert!(matches!(classify(json!({"ok": true})), Outcome::Ok(_)));
}

#[test]
fn other_first_attach_refusals_end_the_view() {
    let (follower, updates, _) = scripted(Vec::new());
    follower.run();
    assert!(matches!(
        updates.try_iter().collect::<Vec<_>>()[..],
        [Update::Refused(_)]
    ));
}

#[test]
fn closing_a_subscription_stops_after_an_empty_inflight_poll() {
    let (mut follower, updates, _) = scripted(Vec::new());
    let mut subscription = Some(Subscription(Arc::clone(&follower.cancelled)));
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    follower.reader = Box::new(move |request| {
        assert_eq!(request["op"], "live");
        assert_eq!(
            counted.fetch_add(1, Ordering::Relaxed),
            0,
            "closed view polled again"
        );
        drop(subscription.take());
        Outcome::Ok(json!({"cursor":{"seq":1}, "items":[], "durable_changed":false}))
    });
    follower.follow();
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(updates.try_iter().next().is_none());
}

#[test]
fn closing_a_view_drains_queued_writes_with_the_original_ids() {
    let (commands, pending) = std::sync::mpsc::channel();
    let (updates, received) = std::sync::mpsc::sync_channel(1);
    drop(received);
    for action in 1..=2 {
        commands
            .send(Command::Write {
                action,
                id: format!("request-{action}"),
                label: "send".into(),
                request: json!({"body":action}),
            })
            .unwrap();
        commands.send(Command::History { before: 1 }).unwrap();
    }
    drop(commands);
    let mut delivered = Vec::new();
    work(
        Path::new("/unused"),
        "a",
        &pending,
        &updates,
        &AtomicBool::new(true),
        |id, request| {
            delivered.push((id.to_owned(), request.clone()));
            Ok(json!({}))
        },
    );
    assert_eq!(
        delivered,
        [
            ("request-1".into(), json!({"body":1})),
            ("request-2".into(), json!({"body":2}))
        ]
    );
}

#[test]
fn child_notices_follow_pages_and_survive_equal_counts_and_removals() {
    let (mut follower, updates, requests) = scripted(Vec::new());
    let pages = [
        // Initial children span short pages (for example a byte-budget cut).
        (0, json!({"agents":[{"id":"old-1"}], "next_after":200})),
        (200, json!({"agents":[{"id":"old-2"}], "next_after":201})),
        (201, json!({"agents":[], "next_after":null})),
        // A new child beyond that baseline is announced.
        (201, json!({"agents":[{"id":"new-1"}], "next_after":202})),
        (202, json!({"agents":[], "next_after":null})),
        // One old child disappears and another arrives: count stays 3.
        (202, json!({"agents":[{"id":"new-2"}], "next_after":203})),
        (203, json!({"agents":[], "next_after":null})),
    ];
    let log = Arc::clone(&requests);
    let mut pages = pages.into_iter();
    follower.reader = Box::new(move |request| {
        log.lock().unwrap().push(request.clone());
        let (after, reply) = pages.next().expect("unexpected child listing");
        assert_eq!(request["after"], after);
        Outcome::Ok(reply)
    });
    assert!(follower.after_reply((None, 2)));
    assert!(updates.try_iter().next().is_none());
    assert!(follower.after_reply((None, 3)));
    assert!(follower.after_reply((None, 3)));
    let ids: Vec<String> = updates
        .try_iter()
        .flat_map(|update| match update {
            Update::Children(rows) => rows
                .into_iter()
                .map(|row| row["id"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>(),
            _ => panic!("unexpected update"),
        })
        .collect();
    assert_eq!(ids, ["new-1", "new-2"]);
    assert_eq!(follower.child_after, Some(203));
    assert!(follower.after_reply((None, 0)));
    assert_eq!(follower.child_after, Some(203));
}
