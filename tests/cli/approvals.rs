//! Approval boundaries preserve private source context and bounded observation exits.
use super::support;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixListener,
    path::PathBuf,
    process::Output,
    thread::JoinHandle,
    time::{Duration, Instant},
};

struct Socket {
    root: tempfile::TempDir,
    source: PathBuf,
    worker: JoinHandle<()>,
}
impl Socket {
    fn new(reply: Option<Value>) -> Self {
        let root = tempfile::tempdir_in("/tmp").unwrap();
        let source = root.path().join("PRIVATE\nanswer.json");
        std::fs::write(&source, r#"{"text":"public"}"#).unwrap();
        std::fs::create_dir(root.path().join("runtime")).unwrap();
        let listener = UnixListener::bind(hive::runtime::socket_path(root.path())).unwrap();
        listener.set_nonblocking(true).unwrap();
        let consumed = source.clone();
        let worker = std::thread::spawn(move || {
            let mut stream = support::wait::wait_until(
                Duration::from_secs(5),
                Duration::from_millis(5),
                "approval request",
                || match listener.accept() {
                    Ok((stream, _)) => Some(stream),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => None,
                    Err(error) => panic!("{error}"),
                },
            );
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = String::new();
            BufReader::new(&stream).read_line(&mut request).unwrap();
            assert!(!request.contains("PRIVATE"));
            std::fs::remove_file(consumed).unwrap();
            if let Some(reply) = reply {
                writeln!(stream, "{reply}").unwrap();
            } else {
                std::thread::sleep(Duration::from_millis(1200));
            }
        });
        Self {
            root,
            source,
            worker,
        }
    }
    fn run(self, args: &[&str]) -> Output {
        let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
            .arg("--state-dir")
            .arg(self.root.path())
            .args(args.iter().map(|arg| {
                if *arg == "FILE" {
                    self.source.as_os_str()
                } else {
                    std::ffi::OsStr::new(arg)
                }
            }))
            .output()
            .unwrap();
        self.worker.join().unwrap();
        output
    }
}
fn input_error(pointer: &str) -> Value {
    json!({"request_id":"answer-retry", "error":{"code":"invalid_value", "message":"Invalid approval input.",
        "field":pointer, "expected":"valid input", "examples":[r#"{"op":"approvals","limit":100}"#]}})
}

#[test]
fn remote_response_data_source_is_local_private_and_never_reopened() {
    for file in [false, true] {
        let output = Socket::new(Some(input_error("/content"))).run(&[
            "respond",
            "00000000-0000-4000-8000-000000000001",
            "--accept",
            "--request-id",
            "answer-retry",
            if file { "--data-file" } else { "--data" },
            if file { "FILE" } else { r#"{"text":"public"}"# },
        ]);
        assert_eq!(output.status.code(), Some(2));
        let wire: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            wire["error"]["field"],
            if file { "--data-file" } else { "--data" }
        );
        assert_eq!(wire["request_id"], "answer-retry");
        assert!(!wire.to_string().contains("PRIVATE"));
        let human = String::from_utf8(output.stderr).unwrap();
        assert_eq!(human.contains("Path:"), file);
        assert_eq!(human.contains("PRIVATE\\nanswer.json"), file);
        assert!(!human.contains("PRIVATE\nanswer.json"));
    }
}

#[test]
fn approval_poll_input_errors_exit_two_but_timeout_and_operational_errors_exit_one() {
    let mut input = input_error("/agent");
    input.as_object_mut().unwrap().remove("request_id");
    for (reply, code, exit) in [
        (Some(input), "invalid_value", 2),
        (
            Some(json!({"error":{"code":"provider_failed","message":""}})),
            "provider_failed",
            1,
        ),
        (None, "timeout", 1),
    ] {
        let start = Instant::now();
        let output = Socket::new(reply).run(&["approvals", "--wait", "--timeout", "1"]);
        assert!(start.elapsed() < Duration::from_secs(3));
        assert_eq!(
            output.status.code(),
            Some(exit),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let wire: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(wire["error"]["code"], code);
        if exit == 2 {
            assert_eq!(wire["error"]["field"], "--agent");
        }
    }
}
