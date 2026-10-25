//! Deterministic socket fixture measures batching and failure admission.
use super::*;
use std::{
    io::{BufRead, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

struct CollectorServer {
    _directory: tempfile::TempDir,
    credential: PathBuf,
    capabilities: Arc<AtomicUsize>,
    captures: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl CollectorServer {
    fn new(reject: bool) -> Self {
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        let socket = directory.path().join("collector.sock");
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
        let capabilities = Arc::new(AtomicUsize::new(0));
        let captures = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (count, messages, stopped) = (capabilities.clone(), captures.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let mut socket = match listener.accept() {
                    Ok((socket, _)) => socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(error) => panic!("collector fixture: {error}"),
                };
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut frame = String::new();
                if std::io::BufReader::new(&socket)
                    .read_line(&mut frame)
                    .unwrap()
                    == 0
                {
                    // A daemon shutting down may close a connection unsent.
                    continue;
                }
                let request: Value = serde_json::from_str(&frame).unwrap();
                let operation = &request["operation"];
                let reply = match operation["op"].as_str().unwrap() {
                    "capabilities" => {
                        count.fetch_add(1, Ordering::Relaxed);
                        json!({"protocol":1,"status":"ok","result":{"protocols":{"minimum":1,"maximum":1},"reviewer":false}})
                    }
                    "capture" => {
                        messages
                            .lock()
                            .unwrap()
                            .push(operation["request_id"].as_str().unwrap().into());
                        if reject {
                            // Revocation makes Registry::authenticate return forbidden.
                            json!({"protocol":1,"status":"error","error": ::comb::Error::forbidden()})
                        } else {
                            json!({"protocol":1,"status":"ok","result":{"captured":true}})
                        }
                    }
                    other => panic!("unexpected collector operation {other}"),
                };
                let _ = writeln!(socket, "{reply}");
            }
        });
        Self {
            _directory: directory,
            credential,
            capabilities,
            captures,
            stop,
            thread: Some(thread),
        }
    }

    fn queue(&self, f: &mut Harness, count: usize) {
        f.edit_config(|config| {
            config["comb_collectors"] =
                json!({"shared":{"credential_file":self.credential,"sources":["correction"]}});
        });
        f.start();
        // Pin the intended identity at startup, then hold delivery while seeding
        // the queue. No capture/network timing influences the initial batch size.
        let hidden = self.credential.with_extension("hidden");
        fs::rename(&self.credential, &hidden).unwrap();
        for _ in 0..count {
            f.ok(json!({"op":"evidence","kind":"correction","summary":"Public mock measurement."}));
        }
        assert_eq!(
            f.ok(json!({"op":"collectors"}))["collectors"][0]["pending"],
            count
        );
        fs::rename(hidden, &self.credential).unwrap();
    }
}
impl Drop for CollectorServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let result = thread.join();
            if !std::thread::panicking() {
                result.unwrap();
            }
        }
    }
}

#[test]
fn collector_batches_drain_1024_items_without_per_item_poll_or_authentication() {
    let server = CollectorServer::new(false);
    let mut f = Harness::builder(Provider::Codex).build();
    server.queue(&mut f, 1024);
    // Delivery first waits out the retry delay set while the credential was
    // hidden; the drain is timed from its first authentication.
    support::wait::wait_until(
        support::wait::TEST_TIMEOUT,
        Duration::from_millis(50),
        "collector retry delay",
        || (server.capabilities.load(Ordering::Relaxed) > 0).then_some(()),
    );
    let start = Instant::now();
    support::wait::wait_until(
        Duration::from_secs(20),
        Duration::from_millis(50),
        "batched collector drain",
        || (f.ok(json!({"op":"collectors"}))["collectors"][0]["pending"] == 0).then_some(()),
    );
    let capabilities = server.capabilities.load(Ordering::Relaxed);
    let captures = server.captures.lock().unwrap();
    assert_eq!(captures.len(), 1024);
    assert_eq!(
        captures
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        1024
    );
    // Normally exactly 32. The one-second admission bound may split a batch on
    // a loaded CI host, without regressing to one authentication per item.
    assert!(
        (32..=40).contains(&capabilities),
        "{capabilities} capabilities calls"
    );
    println!(
        "1024 captures, {capabilities} capability checks, drain after the retry delay {:?}",
        start.elapsed()
    );
    f.shutdown();
}

#[test]
fn collector_capture_failure_stops_batch_before_pinning_another_item() {
    let server = CollectorServer::new(true);
    let mut f = Harness::builder(Provider::Codex).build();
    server.queue(&mut f, 5);
    support::wait::wait_until(
        Duration::from_secs(8),
        Duration::from_millis(25),
        "failed capture backoff",
        || {
            let status = f.ok(json!({"op":"collectors"}));
            (status["collectors"][0]["last_error"] == ::comb::ErrorCode::Forbidden.as_str())
                .then_some(())
        },
    );
    // The worker must remain backed off, not immediately pin subsequent items.
    std::thread::sleep(Duration::from_millis(500));
    let status = f.ok(json!({"op":"collectors"}));
    assert_eq!(status["collectors"][0]["pending"], 5);
    assert_eq!(status["collectors"][0]["uncertain"], 1);
    assert_eq!(server.capabilities.load(Ordering::Relaxed), 1);
    assert_eq!(server.captures.lock().unwrap().len(), 1);
    f.shutdown();
}

#[path = "build_variants.rs"]
mod build_variants;
mod inputs;

#[test]
fn synthetic_release_collector_outbox_survives_upgrade_and_drains_once() {
    let server = CollectorServer::new(false);
    let mut f = Harness::new();
    f.edit_config(|config| {
        config["comb_collectors"] =
            json!({"shared":{"credential_file":server.credential,"sources":["correction"]}});
    });
    f.start();
    let hidden = server.credential.with_extension("hidden");
    fs::rename(&server.credential, &hidden).unwrap();
    let queued: Vec<_> = (0..3).map(|i| {
        let id = format!("corpus-evidence-{i}");
        let request = json!({"op":"evidence","kind":"correction","summary":format!("Synthetic corpus evidence {i}.")});
        let response = f.call_id(&id, request.clone());
        assert!(response["error"].is_null(), "{response}");
        (id, request, response)
    }).collect();
    assert_eq!(
        f.ok(json!({"op":"collectors"}))["collectors"][0]["pending"],
        3
    );
    f.shutdown();
    let state = State::new(f.root.clone()).unwrap();
    for dry_run in [true, false] {
        assert_eq!(state.upgrade(dry_run).unwrap().status, "current");
    }
    fs::rename(hidden, &server.credential).unwrap();
    f.start();
    for (id, request, response) in queued {
        assert_eq!(f.call_id(&id, request), response);
    }
    support::wait::wait_until(
        Duration::from_secs(15),
        Duration::from_millis(25),
        "upgraded outbox drain",
        || (f.ok(json!({"op":"collectors"}))["collectors"][0]["pending"] == 0).then_some(()),
    );
    f.shutdown();
    f.start();
    assert_eq!(
        f.ok(json!({"op":"collectors"}))["collectors"][0]["pending"],
        0
    );
    f.shutdown();
    let captures = server.captures.lock().unwrap();
    assert_eq!(captures.len(), 3);
    assert_eq!(
        captures
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3
    );
}

provider_matrix!(collector_hosted_evidence_field_errors, evidence_errors_case);
fn evidence_errors_case(provider: Provider) {
    let mut f = Harness::builder(provider).build();
    f.start();
    let invalid = [
        ("summary", json!(""), "/summary", "invalid_value"),
        (
            "summary",
            json!("x".repeat(8193)),
            "/summary",
            "out_of_range",
        ),
        ("files", json!(["good.rs", ""]), "/files/1", "invalid_value"),
        (
            "files",
            json!(["x".repeat(513)]),
            "/files/0",
            "out_of_range",
        ),
        ("files", json!(vec!["f"; 65]), "/files", "out_of_range"),
        ("commit", json!(""), "/commit", "invalid_value"),
    ];
    let mut actions = Vec::new();
    for (key, value, _, _) in &invalid {
        let mut args = json!({"kind":"review", "summary":"Public review.", "verdict":"findings", "request_id":"same-retry"});
        args[key] = value.clone();
        actions.push(json!({"name":"hive_evidence", "arguments":args}));
    }
    let good = json!({"name":"hive_evidence", "arguments":{"kind":"review", "summary":"Public review.", "verdict":"no_findings", "request_id":"same-retry"}});
    actions.extend([good.clone(), good]);
    let agent = spawn(&f, provider, false, json!(actions));
    let replies = results(&f, &agent, 1);
    assert_eq!(replies.len(), invalid.len() + 2);
    for (reply, (_, _, field, code)) in replies.iter().zip(&invalid) {
        assert_eq!(reply["error"]["code"], *code, "{reply}");
        assert_eq!(reply["error"]["field"], *field, "{reply}");
        assert!(reply["error"].get("received").is_none());
        assert!(!reply["error"]["examples"].as_array().unwrap().is_empty());
    }
    assert!(replies[invalid.len()]["error"].is_null());
    assert_eq!(
        replies[invalid.len()],
        replies[invalid.len() + 1],
        "invalid calls do not consume the retry ID; valid replay has one admission"
    );
    f.shutdown();
}
