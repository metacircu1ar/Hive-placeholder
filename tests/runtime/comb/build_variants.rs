//! Run with a separately built release core binary; never unify Cargo features.
use super::*;

fn snapshot(f: &Harness, query: &str) -> Vec<Vec<String>> {
    let db = rusqlite::Connection::open(f.root.join("runtime/runtime.sqlite")).unwrap();
    let mut statement = db.prepare(query).unwrap();
    let columns = statement.column_count();
    statement
        .query_map([], |row| {
            (0..columns)
                .map(|i| {
                    row.get::<_, rusqlite::types::Value>(i)
                        .map(|v| format!("{v:?}"))
                })
                .collect()
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
}
fn core_cli(f: &Harness, binary: &std::path::Path, args: &[&str]) -> Value {
    let before = Instant::now();
    let output = support::command(binary)
        .arg("--state-dir")
        .arg(&f.root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        before.elapsed() < Duration::from_secs(5),
        "held observer did not return promptly"
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| panic!("{e}: {output:?}"))
}

struct Switch {
    comb: CombService,
    collector: CollectorServer,
    f: Harness,
    writer: String,
    capable_config: Vec<u8>,
    request: Value,
    accepted: Value,
    batch: Value,
    batch_accepted: Value,
    joined: Value,
    scheduled: Value,
    hidden: PathBuf,
}
impl Switch {
    fn new(binary: &std::path::Path) -> Self {
        let comb = CombService::new();
        let collector = CollectorServer::new(false);
        let mut f = Harness::new();
        f.edit_config(|config| {
            config["comb"] = json!({"credential_file":comb.credential});
            config["comb_collectors"] =
                json!({"shared":{"credential_file":collector.credential,"sources":["correction"]}});
        });
        let capable_config = fs::read(&f.config).unwrap();
        f.start_binary(binary, false);
        let writer = spawn(
            &f,
            Provider::Codex,
            false,
            json!([
                {"name":"comb_put","arguments":{"slug":"concepts/variant","markdown":"---\ntitle: Variant\n---\nRetained knowledge"}}
            ]),
        );
        assert!(results(&f, &writer, 1)[0]["error"].is_null());
        f.ok(json!({"op":"stop","target":writer}));
        f.wait(&writer, |a| a["state"] == "stopped");
        let request = json!({"op":"send","target":writer,"body":"reply:retained"});
        let accepted = f.call_id("same-request", request.clone());
        let batch = json!({"op":"batch","operations":[{"op":"send","target":writer,"body":"reply:batch","request_id":"batch-member"}]});
        let batch_accepted = f.call_id("same-batch", batch.clone());
        let joined = f.ok(json!({"op":"batch","join":{},"operations":[{"op":"send","target":writer,"body":"reply:joined","request_id":"join-member"}]}));
        let scheduled =
            f.ok(json!({"op":"send","target":writer,"body":"reply:scheduled","delay_seconds":300}));
        let hidden = collector.credential.with_extension("hidden");
        fs::rename(&collector.credential, &hidden).unwrap();
        f.ok(json!({"op":"evidence","kind":"correction","summary":"Retained across variants"}));
        f.shutdown();
        Self {
            comb,
            collector,
            f,
            writer,
            capable_config,
            request,
            accepted,
            batch,
            batch_accepted,
            joined,
            scheduled,
            hidden,
        }
    }
    fn check_core(&mut self, binary: &std::path::Path) {
        let Self {
            f,
            collector,
            writer,
            request,
            accepted,
            batch,
            batch_accepted,
            joined,
            scheduled,
            hidden,
            ..
        } = self;
        let rows = snapshot(f, "SELECT * FROM comb_outbox ORDER BY id");
        let destinations = snapshot(f, "SELECT * FROM comb_collectors ORDER BY name");
        let input_query = "SELECT id,body,state,not_before FROM messages WHERE state IN ('accepted','scheduled') ORDER BY seq";
        let inputs = snapshot(f, input_query);
        let identity_query = "SELECT id,auth,json_extract(data,'$.auth_generation'),json_extract(data,'$.comb') FROM agents ORDER BY id";
        let identities = snapshot(f, identity_query);
        assert_eq!(rows.len(), 1);
        fs::rename(hidden, &collector.credential).unwrap();
        f.edit_config(|config| {
            config.as_object_mut().unwrap().remove("comb");
            config.as_object_mut().unwrap().remove("comb_collectors");
        });
        f.start_binary(binary, false);
        let status = f.ok(json!({"op":"ping"}));
        assert_eq!(status["compiled_features"], json!([]));
        assert_eq!(status["comb_backlog"]["rows"], 1);
        assert_eq!(status["comb_backlog"]["delivery_supported"], false);
        assert_eq!(f.status(writer)["held_for_build"], "comb");
        let hidden_provider = f.binary.with_extension("hidden");
        fs::rename(&f.binary, &hidden_provider).unwrap();
        f.expect_err(
            json!({"op":"resume","target":writer}),
            "unsupported_operation",
        );
        f.expect_err(
            json!({"op":"spawn","from_agent":writer,"role":"implementor","body":"must not fork"}),
            "unsupported_operation",
        );
        f.expect_err(json!({"op":"batch","join":{},"operations":[{"op":"send","target":writer,"body":"must roll back","request_id":"rejected"}]}), "unsupported_operation");
        fs::rename(hidden_provider, &f.binary).unwrap();
        let replay = f.call_id("same-request", request.clone());
        assert_eq!(replay["message_id"], accepted["message_id"]);
        assert_eq!(replay["held_for_build"], "comb");
        let batch_replay = f.call_id("same-batch", batch.clone());
        assert_eq!(
            batch_replay["results"][0]["message_id"],
            batch_accepted["results"][0]["message_id"]
        );
        assert_eq!(batch_replay["results"][0]["held_for_build"], "comb");
        observe(f, binary, writer, accepted, joined, scheduled);
        assert_eq!(snapshot(f, input_query), inputs);
        assert_eq!(snapshot(f, identity_query), identities);
        assert_eq!(snapshot(f, "SELECT * FROM comb_outbox ORDER BY id"), rows);
        assert_eq!(
            snapshot(f, "SELECT * FROM comb_collectors ORDER BY name"),
            destinations
        );
        assert!(collector.captures.lock().unwrap().is_empty());
        fresh_call(f, binary, writer);
        let unrelated = f.spawn("reply:core works");
        super::super::super::claude::answered(f, &unrelated, "reply:core works");
        f.shutdown();
    }
    fn restore(&mut self, binary: &std::path::Path) {
        let Self {
            f,
            capable_config,
            request,
            accepted,
            batch,
            batch_accepted,
            writer,
            collector,
            comb,
            ..
        } = self;
        fs::write(&f.config, &*capable_config).unwrap();
        f.start_binary(binary, false);
        assert_eq!(f.call_id("same-request", request.clone()), *accepted);
        assert_eq!(f.call_id("same-batch", batch.clone()), *batch_accepted);
        f.ok(json!({"op":"resume","target":writer}));
        super::super::super::claude::answered(f, writer, "reply:core-call");
        support::wait::wait_until(
            Duration::from_secs(20),
            Duration::from_millis(25),
            "restored collector",
            || (collector.captures.lock().unwrap().len() == 1).then_some(()),
        );
        let events = f.events(writer);
        for body in [
            "reply:retained",
            "reply:batch",
            "reply:joined",
            "reply:core-call",
        ] {
            assert_eq!(
                events
                    .events
                    .iter()
                    .filter(|event| event.data["text"] == body)
                    .count(),
                1
            );
        }
        assert!(f.status(writer).get("held_for_build").is_none());
        assert_eq!(
            comb.get("concepts/variant")["page"]["metadata"]["title"],
            "Variant"
        );
        f.shutdown();
    }
}
fn observe(
    f: &Harness,
    binary: &std::path::Path,
    writer: &str,
    accepted: &Value,
    joined: &Value,
    scheduled: &Value,
) {
    let waiting = core_cli(
        f,
        binary,
        &[
            "wait",
            "--message",
            accepted["message_id"].as_str().unwrap(),
            "--timeout",
            "30",
        ],
    );
    assert_eq!(waiting["outcome"], "held");
    let waiting = core_cli(
        f,
        binary,
        &[
            "wait",
            "--join",
            joined["join"]["join_id"].as_str().unwrap(),
            "--timeout",
            "30",
        ],
    );
    assert_eq!(waiting["wait"], "held");
    let cursor = f.ok(json!({"op":"read","target":writer}))["cursor"].to_string();
    let waiting = core_cli(
        f,
        binary,
        &[
            "wait",
            writer,
            "--until",
            "turn-finished",
            "--after",
            &cursor,
            "--timeout",
            "30",
        ],
    );
    assert_eq!(waiting["wait"], "held");
    let waiting = core_cli(
        f,
        binary,
        &[
            "wait",
            "--message",
            scheduled["message_id"].as_str().unwrap(),
            "--timeout",
            "30",
        ],
    );
    assert_eq!(waiting["outcome"], "held");
}
fn fresh_call(f: &Harness, binary: &std::path::Path, writer: &str) {
    let body = f.tmp.path().join("core-call.txt");
    fs::write(&body, "reply:core-call").unwrap();
    let args = [
        "send",
        writer,
        "--body-file",
        body.to_str().unwrap(),
        "--request-id",
        "core-call",
        "--wait",
        "--timeout",
        "30",
    ];
    let first = core_cli(f, binary, &args);
    assert_eq!(first["outcome"], "held");
    assert_eq!(first["result"]["held_for_build"], "comb");
    assert_eq!(
        core_cli(f, binary, &args)["message_id"],
        first["message_id"]
    );
}

fn release_binary(variable: &str, variant: &str) -> PathBuf {
    let binary = PathBuf::from(std::env::var_os(variable).expect(variable));
    assert!(
        binary.is_absolute()
            && binary
                .components()
                .any(|part| part.as_os_str() == "release")
    );
    let version = support::command(&binary).arg("--version").output().unwrap();
    assert!(version.status.success());
    assert!(String::from_utf8(version.stdout).unwrap().contains(variant));
    binary
}

#[test]
#[ignore = "requires HIVE_CORE_BINARY and HIVE_DEFAULT_BINARY from separate release builds"]
fn capable_core_capable_preserves_login_inputs_collectors_and_live_replays() {
    let core = release_binary("HIVE_CORE_BINARY", "(core)");
    let capable = release_binary("HIVE_DEFAULT_BINARY", "(comb)");
    let mut fixture = Switch::new(&capable);
    fixture.check_core(&core);
    fixture.restore(&capable);
}
