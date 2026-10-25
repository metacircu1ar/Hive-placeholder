use super::*;

#[test]
fn control_connection_limit_backpressures_instead_of_dropping_requests() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    let mut f = Harness::new();
    f.start();
    let socket = runtime::socket_path(&f.root);
    // These earlier sockets hold every live request slot while waiting for a
    // request line. The following complete request stays in the listen backlog.
    let mut held: Vec<_> = (0..64)
        .map(|_| UnixStream::connect(&socket).unwrap())
        .collect();
    let mut extra = UnixStream::connect(&socket).unwrap();
    extra
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    writeln!(
        extra,
        "{}",
        json!({"protocol":1,"id":"overflow","args":{"op":"ping"}})
    )
    .unwrap();
    let mut reader = BufReader::new(extra);
    let error = reader.read_line(&mut String::new()).unwrap_err();
    assert!(matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    ));
    writeln!(
        held[0],
        "{}",
        json!({"protocol":1,"id":"release-slot","args":{"op":"ping"}})
    )
    .unwrap();
    reader
        .get_ref()
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut line = String::new();
    assert!(reader.read_line(&mut line).unwrap() > 0);
    let response: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(response["status"], "running");
    assert!(response.get("error").is_none(), "{response}");
    drop(held);
    drop(reader);
    f.shutdown();
}

mod ping_pong;

#[test]
fn hundred_agent_fanout_paginates_all_messages_and_records_owned_processes() {
    let mut f = Harness::new();
    f.edit_config(|config| {
        config["roles"]["loadping"] = json!({"auth":"main", "model":"gpt-5.6-luna",
            "effort":"medium", "instructions":"role.md", "lifetime":"independent",
            "sandbox":"read-only"});
    });
    f.set_scenario(
        Provider::Codex,
        json!({"ping_count":100,"ping_role":"loadping"}),
    );
    f.start();
    let id = f.spawn("ping pong scenario");
    f.wait_timeout(&id, Duration::from_secs(90), |a| {
        a["task_outcome"] == "completed" && a["state"] == "stopped"
    });
    let all = f.call(json!({"op":"agents"}));
    assert_eq!(all["agents"].as_array().unwrap().len(), 101);
    let (providers, mut startup_events) = shared_fanout_evidence(&f, &id, &all);
    assert_message_pagination(&f, &id);
    startup_events.sort_by_key(|e| e["seq"].as_u64().unwrap());
    let mut initializing = 0;
    for e in startup_events {
        if e["type"] == "execution_starting" {
            initializing += 1;
        } else {
            initializing -= 1;
        }
        assert!(
            (0..=8).contains(&initializing),
            "Startup concurrency exceeded limit"
        );
    }
    assert_eq!(initializing, 0);
    assert!(providers < 101, "Default sharing never reused a provider");
    f.shutdown();
    let db = rusqlite::Connection::open(f.root.join("runtime/runtime.sqlite")).unwrap();
    let invalid: i64 = db.query_row("SELECT count(*) FROM (SELECT provider FROM provider_events GROUP BY provider HAVING sum(kind='started') != 1 OR sum(kind='shutdown') != 1 OR min(CASE WHEN kind='started' THEN seq END) >= min(CASE WHEN kind='shutdown' THEN seq END))", [], |r| r.get(0)).unwrap();
    assert_eq!(invalid, 0);
    assert_eq!(f.reports().len(), 101);
    assert!(f.reports().iter().all(|r| r["confirmed"] == true));
}

fn shared_fanout_evidence(f: &Harness, id: &str, all: &Value) -> (usize, Vec<Value>) {
    let mut providers = std::collections::HashMap::new();
    let mut threads = std::collections::HashSet::new();
    let mut startup_events = Vec::new();
    for a in all["agents"].as_array().unwrap() {
        let aid = a["id"].as_str().unwrap();
        f.wait(aid, |a| a["state"] == "stopped");
        if aid != id {
            assert_eq!(a["model"], "gpt-5.6-luna");
            assert_eq!(a["effort"], "medium");
        }
        let events = f.read(aid);
        startup_events.extend(
            events["events"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|e| e["type"] == "execution_starting" || e["type"] == "session_ready")
                .cloned(),
        );
        let event = events["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["type"] == "provider_started")
            .unwrap();
        let identity = event["data"].clone();
        if let Some(previous) = providers.insert(
            identity["provider_id"].as_str().unwrap().to_owned(),
            identity.clone(),
        ) {
            assert_eq!(previous, identity);
        }
        assert!(threads.insert(a["thread_id"].as_str().unwrap().to_owned()));
        assert_eq!(event["execution_id"], a["execution_id"]);
        let starts = events["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["type"] == "execution_starting")
            .collect::<Vec<_>>();
        ping_pong::check_releases(&json!({"events":{aid:events["events"]}}), aid, &starts);
    }
    (providers.len(), startup_events)
}

#[test]
fn slow_reader_receives_a_complete_large_daemon_response() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    let mut f = Harness::new();
    f.start();
    let id = f.spawn("idle");
    f.settled(&id);
    f.ok(json!({"op":"stop","target":id}));
    f.wait(&id, |a| a["state"] == "stopped");
    let body = "x".repeat(16384);
    for _ in 0..32 {
        assert_eq!(
            f.call(json!({"op":"send","target":id,"body":body}))["delivery_state"],
            "accepted"
        );
    }
    let mut stream = UnixStream::connect(runtime::socket_path(&f.root)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    writeln!(
        stream,
        "{}",
        json!({"protocol":hive::compatibility::CONTROL_PROTOCOL,
            "id":"slow-reader","args":{"op":"messages","target":id,"limit":200}})
    )
    .unwrap();
    // Force server-side backpressure. Nonblocking accepted sockets used to stop
    // writing at WouldBlock and close with a partial, invalid JSON response.
    std::thread::sleep(Duration::from_millis(100));
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    let response: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(response["messages"].as_array().unwrap().len(), 33);
    assert_eq!(
        response["messages"][0]["body"].as_str().unwrap().len(),
        16384
    );
    f.shutdown();
}

fn assert_message_pagination(f: &Harness, id: &str) {
    let mut before = None;
    let mut seen = std::collections::HashSet::new();
    let mut last = u64::MAX;
    loop {
        let page = f.call(json!({"op":"messages","target":id,"before":before,"limit":17}));
        let rows = page["messages"].as_array().unwrap();
        assert!(rows.len() <= 17);
        if rows.is_empty() {
            assert!(page["next_before"].is_null());
            break;
        }
        for m in rows {
            let seq = m["seq"].as_u64().unwrap();
            assert!(seq < last);
            last = seq;
            assert!(seen.insert(m["id"].as_str().unwrap().to_string()));
            assert_eq!(m["state"], "completed");
        }
        before = page["next_before"].as_u64();
        assert_eq!(before, Some(last));
    }
    assert_eq!(seen.len(), 401);
}
