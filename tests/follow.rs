#![allow(
    clippy::disallowed_methods,
    reason = "Raw writes are gated by the exact the reviewed maintainer write inventory inventory"
)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // Owned socket/child fixtures fail on setup errors.

mod support;

use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        net::{UnixListener, UnixStream},
        process::ExitStatusExt,
    },
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

const TEST_WAIT: Duration = Duration::from_secs(5);
const TEST_POLL: Duration = Duration::from_millis(5);

struct Socket {
    root: tempfile::TempDir,
    listener: UnixListener,
}

impl Socket {
    fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("hl-follow-")
            .tempdir_in("/tmp")
            .unwrap();
        fs::create_dir(root.path().join("runtime")).unwrap();
        let listener = UnixListener::bind(root.path().join("runtime/control.sock")).unwrap();
        listener.set_nonblocking(true).unwrap();
        Self { root, listener }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = support::command(env!("CARGO_BIN_EXE_hivectl"));
        cmd.arg("--state-dir")
            .arg(self.root.path())
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }

    fn request(&self) -> (UnixStream, Value) {
        let (stream, _) = support::wait::wait_until(TEST_WAIT, TEST_POLL, "follow request", || {
            match self.listener.accept() {
                Ok(connection) => Some(connection),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => None,
                Err(e) => panic!("accept follow request: {e}"),
            }
        });
        stream.set_nonblocking(false).unwrap();
        stream.set_read_timeout(Some(TEST_WAIT)).unwrap();
        stream.set_write_timeout(Some(TEST_WAIT)).unwrap();
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).unwrap();
        (stream, serde_json::from_str(&line).unwrap())
    }

    fn respond(&self, after: u64, page: &Value) {
        self.respond_request(
            &json!({"op":"read","target":"a","after":after,"limit":100}),
            page,
        );
    }

    fn respond_request(&self, args: &Value, page: &Value) {
        let (mut stream, request) = self.request();
        assert_eq!(&request["args"], args);
        writeln!(stream, "{page}").unwrap();
    }
}

struct Viewer {
    child: support::ChildGuard,
    lines: Receiver<Value>,
    output: Option<thread::JoinHandle<()>>,
}

impl Viewer {
    fn start(socket: &Socket, after: &str) -> Self {
        let mut child = support::ChildGuard::new(
            socket
                .command(&["read", "a", "--follow", "--after", after])
                .spawn()
                .unwrap(),
        );
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        let output = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                tx.send(serde_json::from_str(&line.unwrap()).unwrap())
                    .unwrap();
            }
        });
        Self {
            child,
            lines,
            output: Some(output),
        }
    }

    fn line(&self) -> Value {
        self.lines
            .recv_timeout(TEST_WAIT)
            .expect("flushed complete JSONL page")
    }

    fn interrupt(&mut self) {
        let pid = rustix::process::Pid::from_child(&self.child);
        rustix::process::kill_process(pid, rustix::process::Signal::INT).unwrap();
        let status = support::wait::wait_until(TEST_WAIT, TEST_POLL, "interrupted viewer", || {
            self.child.try_wait().unwrap()
        });
        assert_eq!(status.signal(), Some(libc::SIGINT));
        self.output.take().unwrap().join().unwrap();
        assert!(
            self.lines.try_recv().is_err(),
            "no trailing summary or empty page"
        );
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        if let Some(output) = self.output.take() {
            let _ = self.child.kill();
            let _ = self.child.wait();
            let _ = output.join();
        }
    }
}

fn event(sequence: u64, kind: &str, data: Value) -> Value {
    json!({"seq":sequence,"agent_id":"a","execution_id":null,"timestamp":1,"type":kind,"data":data})
}

fn page(events: Vec<Value>, cursor: u64, pruned: u64, expired: bool) -> Value {
    json!({"events":events,"cursor":cursor,"pruned_through":pruned,"history_expired":expired})
}

#[test]
fn follows_empty_history_short_pages_global_gaps_and_later_turns() {
    let socket = Socket::new();
    let mut viewer = Viewer::start(&socket, "5");
    socket.respond(5, &page(vec![], 5, 0, false));
    let first = page(
        vec![event(9, "turn_finished", json!({"output":"first"}))],
        9,
        0,
        false,
    );
    socket.respond(5, &first);
    assert_eq!(viewer.line(), first); // The prior empty page was suppressed.
    let second = page(
        vec![event(20, "turn_finished", json!({"output":"second"}))],
        20,
        0,
        false,
    );
    socket.respond(9, &second);
    assert_eq!(viewer.line(), second);
    let (held, request) = socket.request();
    assert_eq!(request["args"]["after"], 20);
    viewer.interrupt(); // Cancellation during a socket read does not wait 40 seconds.
    drop(held);
}

#[test]
fn reports_pruning_once_and_keeps_large_output_descriptors() {
    let socket = Socket::new();
    let mut viewer = Viewer::start(&socket, "0");
    let gap = page(vec![], 30, 30, true);
    socket.respond(0, &gap);
    assert_eq!(viewer.line(), gap);
    socket.respond(30, &page(vec![], 30, 30, false));
    let artifact = json!({"artifact":{"id":"out","read_target":"artifact:out","bytes":100_000}});
    let next = page(vec![event(35, "turn_finished", artifact)], 35, 30, false);
    socket.respond(30, &next);
    assert_eq!(viewer.line(), next);
    socket.respond(35, &page(vec![], 35, 30, false));
    viewer.interrupt(); // Also cancellable between requests, while idle.
}

fn finished(child: &mut support::ChildGuard) -> (i32, Vec<u8>, String) {
    let status = support::wait::wait_until(TEST_WAIT, TEST_POLL, "viewer exit", || {
        child.try_wait().unwrap()
    });
    let mut stdout = Vec::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut stdout)
        .unwrap();
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    (status.code().unwrap(), stdout, stderr)
}

#[test]
fn follow_operational_errors_use_jsonl_and_human_channels() {
    for (response, code, error) in [
        (
            Some(json!({"error":{"code":"agent_purged","message":"Purged."}})),
            1,
            "agent_purged",
        ),
        (Some(page(vec![], 50, 0, false)), 1, "daemon_protocol_error"),
        (Some(json!([1, 2])), 1, "daemon_protocol_error"),
        (None, 1, "daemon_protocol_error"),
    ] {
        let socket = Socket::new();
        let mut child =
            support::ChildGuard::new(socket.command(&["read", "a", "--follow"]).spawn().unwrap());
        let (mut stream, _) = socket.request();
        if let Some(response) = response {
            writeln!(stream, "{response}").unwrap();
        }
        drop(stream);
        let (exit, stdout, stderr) = finished(&mut child);
        assert_eq!(exit, code);
        let value: Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(value["error"]["code"], error);
        assert_eq!(String::from_utf8_lossy(&stdout).lines().count(), 1);
        assert!(stderr.contains(error));
        assert!(socket.listener.accept().is_err(), "no automatic reconnect");
    }
}

#[test]
fn rejects_artifacts_and_invalid_follow_limits_before_connecting() {
    for args in [
        vec!["read", "artifact:out", "--follow"],
        vec!["read", "a", "--follow", "--limit", "0"],
        vec!["read", "a", "--follow", "--limit", "201"],
    ] {
        let socket = Socket::new();
        let output = socket.command(&args).output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        let error: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            error["error"]["code"],
            if args[1].starts_with("artifact:") {
                "conflicting_arguments"
            } else {
                "out_of_range"
            }
        );
        let human = String::from_utf8(output.stderr).unwrap();
        for example in error["error"]["examples"].as_array().unwrap() {
            assert!(human.contains(example.as_str().unwrap()));
        }
        assert!(socket.listener.accept().is_err());
    }
}

#[test]
fn closed_output_pipe_exits_successfully_without_another_request() {
    let socket = Socket::new();
    let mut child =
        support::ChildGuard::new(socket.command(&["read", "a", "--follow"]).spawn().unwrap());
    drop(child.stdout.take());
    socket.respond(
        0,
        &page(vec![event(1, "turn_finished", json!({}))], 1, 0, false),
    );
    let status = support::wait::wait_until(TEST_WAIT, TEST_POLL, "closed pipe exit", || {
        child.try_wait().unwrap()
    });
    assert!(status.success());
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(stderr.is_empty());
    assert!(socket.listener.accept().is_err());
}

#[test]
fn blocked_output_does_not_request_another_page_and_is_interruptible() {
    let socket = Socket::new();
    let mut child =
        support::ChildGuard::new(socket.command(&["read", "a", "--follow"]).spawn().unwrap());
    // A legal page larger than the pipe's capacity must block before the next read.
    socket.respond(
        0,
        &page(
            vec![event(
                1,
                "turn_finished",
                json!({"output":"x".repeat(700_000)}),
            )],
            1,
            0,
            false,
        ),
    );
    thread::sleep(Duration::from_millis(200));
    assert!(child.try_wait().unwrap().is_none());
    assert!(
        socket.listener.accept().is_err(),
        "no read ahead of a blocked consumer"
    );
    rustix::process::kill_process(
        rustix::process::Pid::from_child(&child),
        rustix::process::Signal::INT,
    )
    .unwrap();
    let status =
        support::wait::wait_until(TEST_WAIT, TEST_POLL, "interrupt blocked output", || {
            child.try_wait().unwrap()
        });
    assert_eq!(status.signal(), Some(libc::SIGINT));
}

#[test]
fn disconnect_after_output_preserves_the_last_complete_page() {
    let socket = Socket::new();
    let mut child =
        support::ChildGuard::new(socket.command(&["read", "a", "--follow"]).spawn().unwrap());
    let expected = page(vec![event(8, "turn_finished", json!({}))], 8, 0, false);
    socket.respond(0, &expected);
    let (stream, request) = socket.request();
    assert_eq!(request["args"]["after"], 8);
    drop(stream);
    let (code, output, error) = finished(&mut child);
    assert_eq!(code, 1);
    let lines: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], expected);
    assert_eq!(lines[1]["error"]["code"], "daemon_protocol_error");
    assert!(error.contains("daemon_protocol_error"));
}

#[test]
fn ordinary_read_still_returns_one_pretty_page() {
    let socket = Socket::new();
    let mut child = support::ChildGuard::new(socket.command(&["read", "a"]).spawn().unwrap());
    let expected = page(vec![event(1, "turn_finished", json!({}))], 1, 0, false);
    socket.respond_request(&json!({"op":"read","target":"a"}), &expected);
    let status = support::wait::wait_until(TEST_WAIT, TEST_POLL, "ordinary read exit", || {
        child.try_wait().unwrap()
    });
    assert!(status.success());
    let mut output = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut output)
        .unwrap();
    assert_eq!(
        output,
        format!("{}\n", serde_json::to_string_pretty(&expected).unwrap())
    );
}

fn input_reply(field: &str) -> Value {
    json!({"request_id":"retry-id","error":{"code":"out_of_range","message":"Invalid pagination arguments.",
        "field":field,"expected":"a bounded page","examples":[r#"{"op":"read","target":"AGENT_ID"}"#]}})
}

fn assert_input_output(output: &std::process::Output, field: &str) {
    assert_eq!(output.status.code(), Some(2));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["request_id"], "retry-id");
    assert_eq!(value["error"]["field"], field);
    assert_eq!(value["error"]["code"], "out_of_range");
    assert!(value["error"].get("received").is_none());
    let human = String::from_utf8(output.stderr.clone()).unwrap();
    assert!(human.contains(field));
    assert!(!human.contains("retry-id"));
    for example in value["error"]["examples"].as_array().unwrap() {
        let example = example.as_str().unwrap();
        assert!(example.starts_with("hivectl "));
        assert!(human.contains(example));
    }
}

#[test]
fn daemon_input_rejections_use_cli_context_and_preserve_retry_identity() {
    for (args, pointer, field) in [
        (vec!["agents"], "/after", "--after"),
        (vec!["agents", "--stalled", "30m"], "/limit", "--limit"),
        (vec!["messages", "a"], "/before", "--before"),
        (vec!["read", "a"], "/target", "TARGET"),
        (
            vec!["output", "00000000-0000-4000-8000-000000000001"],
            "/offset",
            "--offset",
        ),
    ] {
        let socket = Socket::new();
        let mut child = support::ChildGuard::new(socket.command(&args).spawn().unwrap());
        let (mut stream, _) = socket.request();
        writeln!(stream, "{}", input_reply(pointer)).unwrap();
        drop(stream);
        let status = support::wait::wait_until(TEST_WAIT, TEST_POLL, "input rejection", || {
            child.try_wait().unwrap()
        });
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_end(&mut stdout)
            .unwrap();
        child
            .stderr
            .take()
            .unwrap()
            .read_to_end(&mut stderr)
            .unwrap();
        assert_input_output(
            &std::process::Output {
                status,
                stdout,
                stderr,
            },
            field,
        );
        assert!(socket.listener.accept().is_err());
    }
}

#[test]
fn follow_input_rejection_is_one_jsonl_record_before_or_after_pages() {
    for prior_page in [false, true] {
        let socket = Socket::new();
        let mut viewer = Viewer::start(&socket, "0");
        let cursor = if prior_page {
            let first = page(vec![event(1, "turn_finished", json!({}))], 1, 0, false);
            socket.respond(0, &first);
            assert_eq!(viewer.line(), first);
            1
        } else {
            0
        };
        socket.respond(cursor, &input_reply("/limit"));
        let response = viewer.line();
        let status = support::wait::wait_until(TEST_WAIT, TEST_POLL, "input rejection", || {
            viewer.child.try_wait().unwrap()
        });
        let mut stderr = Vec::new();
        viewer
            .child
            .stderr
            .take()
            .unwrap()
            .read_to_end(&mut stderr)
            .unwrap();
        assert_input_output(
            &std::process::Output {
                status,
                stdout: response.to_string().into_bytes(),
                stderr,
            },
            "--limit",
        );
        viewer.output.take().unwrap().join().unwrap();
        assert!(viewer.lines.try_recv().is_err());
        assert!(socket.listener.accept().is_err());
    }
}

#[test]
fn ordinary_read_output_failure_is_not_successful_viewer_cancellation() {
    let socket = Socket::new();
    let mut child = support::ChildGuard::new(socket.command(&["read", "a"]).spawn().unwrap());
    drop(child.stdout.take());
    socket.respond_request(
        &json!({"op":"read","target":"a"}),
        &page(vec![], 0, 0, false),
    );
    let status = support::wait::wait_until(TEST_WAIT, TEST_POLL, "failed result output", || {
        child.try_wait().unwrap()
    });
    assert_eq!(status.code(), Some(1));
    assert!(socket.listener.accept().is_err());
}
