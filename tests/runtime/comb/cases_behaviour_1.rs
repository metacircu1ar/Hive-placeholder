//! Behaviour regressions.
use super::*;

#[test]
fn collector_round_robin_reaches_later_destinations_before_busy_ones_drain() {
    let comb = CombService::new();
    let mut f = Harness::builder(Provider::Codex).build();
    f.edit_config(|config| {
        let destinations: serde_json::Map<String,Value> = (0..16).map(|i| (format!("destination-{i:02}"), json!({"credential_file":comb.credential,"sources":if i < 4 {vec!["correction"]} else {vec!["review"]}}))).collect();
        config["comb_collectors"] = Value::Object(destinations);
    });
    f.start();
    let hidden = comb.credential.with_extension("hidden");
    fs::rename(&comb.credential, &hidden).unwrap();
    for _ in 0..256 {
        f.ok(json!({"op":"evidence","kind":"correction","summary":"Public measurement."}));
    }
    f.ok(json!({"op":"evidence","kind":"review","verdict":"no_findings","summary":"Review complete."}));
    fs::rename(hidden, &comb.credential).unwrap();
    support::wait::wait_until(
        Duration::from_secs(15),
        Duration::from_millis(50),
        "all later destinations delivered",
        || {
            let status = f.ok(json!({"op":"collectors"}));
            let rows = status["collectors"].as_array().unwrap();
            rows[4..].iter().all(|row| row["pending"] == 0).then(|| {
                assert!(
                    rows[..4]
                        .iter()
                        .any(|row| row["pending"].as_i64().unwrap() > 0),
                    "Busy destinations should not need to drain before others run."
                );
            })
        },
    );
    f.shutdown();
}

#[test]
fn collector_fifo_binding_cannot_block_daemon_start_or_shutdown() {
    let mut f = Harness::builder(Provider::Codex).build();
    let fifo = f.tmp.path().join("credential.fifo");
    assert!(
        support::command("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    f.edit_config(|config| {
        config["comb_collectors"] =
            json!({"shared":{"credential_file":fifo,"sources":["correction"]}});
    });
    f.start();
    f.ok(json!({"op":"evidence","kind":"correction","summary":"Public correction retained while unbound."}));
    let status = f.ok(json!({"op":"collectors"}));
    assert_eq!(status["collectors"][0]["last_error"], "identity_unbound");
    assert_eq!(status["collectors"][0]["pending"], 1);
    let start = Instant::now();
    f.shutdown();
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn held_collector_network_does_not_block_agent_or_shutdown() {
    use std::io::BufRead;
    let mut f = Harness::builder(Provider::Codex).build();
    // Unix sockets need a short pathname on macOS.
    let directory = tempfile::tempdir_in("/tmp").unwrap();
    let socket = directory.path().join("held.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let credential = directory.path().join("binding.json");
    Connection {
        format: hive_input::compatibility::Exact,
        socket,
        credential: ::comb::service::auth::Credential {
            comb_id: ::comb::values::CombId::generate(),
            id: ::comb::values::CredentialId::generate(),
            secret: "fixture".into(),
        },
    }
    .write(&credential)
    .unwrap();
    let (seen, entered) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let server = std::thread::spawn(move || {
        let socket = loop {
            if !matches!(
                released.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ) {
                return;
            }
            if let Ok((socket, _)) = listener.accept() {
                break socket;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = std::io::BufReader::new(socket);
        let mut frame = String::new();
        reader.read_line(&mut frame).unwrap();
        seen.send(()).unwrap();
        let _ = released.recv_timeout(Duration::from_secs(15));
    });
    f.edit_config(|config| {
        config["comb_collectors"] =
            json!({"shared":{"credential_file":credential,"sources":["correction"]}});
    });
    f.start();
    f.ok(json!({"op":"evidence","kind":"correction","summary":"Held network evidence."}));
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let id = f.spawn("reply:unrelated progress");
    f.settled(&id);
    assert_eq!(
        f.ok(json!({"op":"collectors"}))["collectors"][0]["pending"],
        1
    );
    let start = Instant::now();
    f.shutdown();
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "Network timeout must not delay shutdown"
    );
    drop(release);
    server.join().unwrap();
}
