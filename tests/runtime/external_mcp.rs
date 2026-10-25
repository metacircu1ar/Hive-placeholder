use super::*;
use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc;

struct Mcp {
    child: support::ChildGuard,
    reader: Option<std::thread::JoinHandle<()>>,
    frames: mpsc::Receiver<Value>,
}
impl Mcp {
    fn start(f: &Harness) -> Self {
        Self::start_protocol(f, "2025-11-25", "2025-11-25")
    }
    fn start_protocol(f: &Harness, offer: &str, selected: &str) -> Self {
        let mut child = support::ChildGuard::new(
            support::command(env!("CARGO_BIN_EXE_hivectl"))
                .arg("--state-dir")
                .arg(&f.root)
                .arg("mcp")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let out = child.stdout.take().unwrap();
        let (tx, frames) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(out).lines() {
                let value = serde_json::from_str(&line.unwrap()).unwrap();
                if tx.send(value).is_err() {
                    break;
                }
            }
        });
        let mut server = Self {
            child,
            frames,
            reader: Some(reader),
        };
        server.send(
            json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
            "protocolVersion":offer,"capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),
        );
        assert_eq!(server.receive()["result"]["protocolVersion"], selected);
        server.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        server
    }
    fn send(&mut self, frame: Value) {
        writeln!(self.child.stdin.as_mut().unwrap(), "{frame}").unwrap();
    }
    fn receive(&self) -> Value {
        self.frames.recv_timeout(Duration::from_secs(5)).unwrap()
    }
    fn reply(&self, id: i32) -> Value {
        loop {
            let frame = self.receive();
            if frame["id"] == id {
                return frame;
            }
            assert_eq!(frame["method"], "notifications/progress", "{frame}");
        }
    }
    fn close(&mut self) {
        self.child.stdin.take();
        let status = support::wait::wait_until(
            Duration::from_secs(2),
            Duration::from_millis(10),
            "MCP EOF observer shutdown",
            || self.child.try_wait().unwrap(),
        );
        assert!(status.success(), "MCP exited unsuccessfully: {status}");
    }
}

#[test]
fn synthetic_release_external_mcp_negotiation_transcripts() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../compat/synthetic-0/transcripts.json")).unwrap();
    let f = Harness::new();
    for case in fixture["mcp"].as_array().unwrap() {
        let mut mcp = Mcp::start_protocol(
            &f,
            case["offer"].as_str().unwrap(),
            case["selected"].as_str().unwrap(),
        );
        mcp.send(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}));
        let reply = mcp.reply(1);
        assert!(
            reply["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["name"] == "hive_call")
        );
        mcp.send(json!({"jsonrpc":"2.0","id":2,"method":"ping"}));
        assert_eq!(mcp.reply(2)["result"], json!({}));
        mcp.close();
    }
}
impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

#[test]
fn external_mcp_drains_a_healthy_pipelined_burst() {
    let f = Harness::new();
    let mut mcp = Mcp::start(&f);
    let mut burst = Vec::new();
    for id in 1..=128 {
        writeln!(
            burst,
            "{}",
            json!({"jsonrpc":"2.0","id":id,"method":"ping"})
        )
        .unwrap();
    }
    mcp.child.stdin.as_mut().unwrap().write_all(&burst).unwrap();
    for id in 1..=128 {
        assert_eq!(mcp.reply(id)["result"], json!({}));
    }
    mcp.close();
    assert!(mcp.child.wait().unwrap().success());
}

#[test]
fn external_mcp_calls_resume_and_cancel_observation_without_cancelling_input() {
    let mut f = Harness::new();
    f.start();
    let agent = f.spawn("hold");
    f.wait(&agent, |a| a["state"] == "running");
    let mut mcp = Mcp::start(&f);
    mcp.send(json!(["2.0", 99, "ping", {}]));
    let invalid = mcp.receive();
    assert_eq!(invalid["error"]["code"], -32600);
    assert!(invalid["id"].is_null());
    mcp.send(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}));
    let catalog = mcp.reply(1);
    assert_eq!(catalog["result"]["tools"].as_array().unwrap().len(), 3);
    mcp.send(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
        "name":"hive_call","arguments":{"target":agent,"body":"reply:mcp","request_id":"mcp-call","timeout":1},
        "_meta":{"progressToken":"progress-2"}}}));
    let progress = mcp.receive();
    assert_eq!(progress["method"], "notifications/progress");
    assert_eq!(progress["params"]["progressToken"], "progress-2");
    let reply = mcp.reply(2);
    let pending = &reply["result"]["structuredContent"];
    assert_eq!(pending["outcome"], "timeout", "{reply}");
    let message = pending["message_id"].as_str().unwrap();
    mcp.send(
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
        "name":"hive_wait_message","arguments":{"message_id":message,"timeout":20}}}),
    );
    // The reader must remain responsive while the long call is active.
    mcp.send(json!({"jsonrpc":"2.0","id":4,"method":"ping"}));
    assert!(mcp.reply(4).get("result").is_some());
    mcp.send(json!({"jsonrpc":"2.0","id":3,"method":"ping"}));
    let duplicate = mcp.receive();
    assert!(duplicate["id"].is_null());
    assert_eq!(duplicate["error"]["code"], -32600);
    mcp.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":3}}));
    mcp.send(
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
        "name":"hive_wait_message","arguments":{"message_id":message,"timeout":10}}}),
    );
    f.ok(json!({"op":"interrupt","target":agent}));
    let reply = mcp.reply(3);
    assert_eq!(
        reply["result"]["structuredContent"]["outcome"], "replied",
        "{reply}"
    );
    assert_eq!(
        reply["result"]["structuredContent"]["result"]["output"]["text"],
        "reply:mcp"
    );
    assert_eq!(
        f.call(json!({"op":"messages","target":agent}))["messages"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    mcp.close();
    assert!(
        matches!(
            mcp.frames.recv_timeout(Duration::from_secs(1)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ),
        "MCP printed a trailing CLI response"
    );
    f.shutdown();
}

#[test]
fn external_mcp_unread_stdout_cannot_block_eof_or_overload_shutdown() {
    for requests in [16, 100] {
        let f = Harness::new();
        let mut child = support::ChildGuard::new(
            support::command(env!("CARGO_BIN_EXE_hivectl"))
                .arg("--state-dir")
                .arg(&f.root)
                .arg("mcp")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        // Deliberately keep stdout open without reading anything from it.
        let _unread = child.stdout.take().unwrap();
        let mut mcp = Mcp {
            child,
            frames: mpsc::channel().1,
            reader: None,
        };
        mcp.send(json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
            "protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}));
        mcp.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        for id in 1..=requests {
            let frame = json!({"jsonrpc":"2.0","id":id,"method":"tools/list"});
            if let Err(error) = writeln!(mcp.child.stdin.as_mut().unwrap(), "{frame}") {
                assert_eq!(requests, 100);
                assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
                break;
            }
        }
        if requests == 16 {
            mcp.close();
        } else {
            // Overflow must also exit with stdin held open, despite Tokio's
            // non-cancellable stdin blocking worker.
            let status = support::wait::wait_until(
                Duration::from_secs(3),
                Duration::from_millis(10),
                "overloaded MCP exits",
                || mcp.child.try_wait().unwrap(),
            );
            assert_eq!(status.code(), Some(1));
        }
    }
}

#[test]
fn external_mcp_bounds_observers_and_eof_leaves_queued_messages_intact() {
    let mut f = Harness::new();
    f.start();
    let agent = f.spawn("hold");
    f.wait(&agent, |a| a["state"] == "running");
    let send = f.call(json!({"op":"send","target":agent,"body":"pending"}));
    let mut mcp = Mcp::start(&f);
    for id in 1..=17 {
        mcp.send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{
            "name":"hive_wait_message","arguments":{"message_id":send["message_id"],"timeout":30}}}));
    }
    let refused = mcp.reply(17);
    assert_eq!(refused["error"]["code"], -32000);
    mcp.close();
    let message = f.call(json!({"op":"message_result","message_id":send["message_id"]}));
    assert_eq!(message["delivery_state"], "accepted");
    f.shutdown();
}

#[test]
fn external_mcp_returns_actionable_input_errors_without_a_daemon() {
    let f = Harness::new();
    let mut mcp = Mcp::start(&f);
    // More requests than observer capacity, reusing the same ID. All must finish
    // immediately even though this state has no running daemon.
    for _ in 0..20 {
        mcp.send(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
            "name":"hive_call","arguments":{"target":"a","body":"secret-canary","request_id":"r","timeout":0}}}));
        let reply = mcp.reply(1);
        assert_eq!(reply["result"]["isError"], true);
        let error = &reply["result"]["structuredContent"]["error"];
        assert_eq!(error["code"], "out_of_range");
        assert_eq!(error["field"], "/arguments/timeout");
        assert!(error.get("received").is_none());
        assert!(!reply.to_string().contains("secret-canary"));
        assert!(!error["examples"].as_array().unwrap().is_empty());
    }
    mcp.send(
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
        "name":"hive_call","arguments":[]}}),
    );
    let reply = mcp.reply(1);
    assert_eq!(reply["error"]["code"], -32602);
    assert_eq!(reply["error"]["data"]["field"], "/arguments");
    mcp.send(json!({"jsonrpc":"2.0","id":2,"method":"ping"}));
    assert_eq!(mcp.reply(2)["result"], json!({}));
    mcp.close();
}
