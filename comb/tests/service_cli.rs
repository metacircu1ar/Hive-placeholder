#![allow(
    clippy::disallowed_methods,
    reason = "Raw writes are gated by the exact the reviewed maintainer write inventory inventory"
)]
//! Actual release binaries, Unix socket transport and MCP stdin/stdout pipes.
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let cleanup = (|| -> std::io::Result<()> {
            if self.0.try_wait()?.is_none() {
                self.0.kill()?;
            }
            self.0.wait()?;
            Ok(())
        })();
        if std::thread::panicking() {
            if let Err(error) = cleanup {
                eprintln!("fixture process cleanup also failed: {error}");
            }
        } else {
            assert!(
                cleanup.is_ok(),
                "fixture process cleanup failed: {cleanup:?}"
            );
        }
    }
}
fn command(root: &Path, socket: Option<&Path>) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_comb"));
    c.arg("--root").arg(root);
    if let Some(socket) = socket {
        c.arg("--socket").arg(socket);
    }
    c
}
fn cli(root: &Path, socket: &Path, args: &[&str], input: Option<&str>) -> TestResult<Value> {
    let socket = (args.first() != Some(&"init")).then_some(socket);
    let mut child = command(root, socket)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(text) = input {
        child
            .stdin
            .take()
            .ok_or("child stdin is not piped")?
            .write_all(text.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}
fn serve(root: &Path, socket: &Path) -> TestResult<Process> {
    let mut process = Process(
        command(root, Some(socket))
            .arg("serve")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::os::unix::net::UnixStream::connect(socket).is_err() {
        assert!(
            process.0.try_wait()?.is_none(),
            "Serving process exited early"
        );
        assert!(Instant::now() < deadline, "Serving process did not listen");
        std::thread::sleep(Duration::from_millis(10));
    }
    cli(root, socket, &["stats"], None)?;
    assert_socket_receipt(socket)?;
    Ok(process)
}
#[test]
fn release_cli_routes_through_service_and_stdio_mcp_uses_the_same_comb() -> TestResult {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")?;
    let root = dir.path();
    let socket = root.join("comb.sock");
    cli(root, &socket, &["init"], None)?;
    let service = serve(root, &socket)?;
    cli(
        root,
        &socket,
        &["put", "components/writer"],
        Some("---\ntitle: Writer\n---\nDurable commits."),
    )?;
    let path = issue_client(root, &socket)?;
    let mut proxy = Process(
        Command::new(env!("CARGO_BIN_EXE_comb"))
            .arg("--socket")
            .arg(&socket)
            .args([
                "mcp",
                "--credential",
                path.to_str().ok_or("credential path is not UTF-8")?,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let replies = exchange(&mut proxy)?;
    verify_capture(root, &socket, &replies)?;
    drop(service);
    // A stale, refused socket is the only post-crash path to offline operation.
    assert_eq!(cli(root, &socket, &["stats"], None)?["pages"], 2);
    // Its durable socket receipt permits restart after the killed child is reaped.
    let _restarted = serve(root, &socket)?;
    assert_eq!(cli(root, &socket, &["stats"], None)?["pages"], 2);
    Ok(())
}

fn issue_client(root: &Path, socket: &Path) -> TestResult<std::path::PathBuf> {
    let path = root.join("client.json");
    let issued = cli(
        root,
        socket,
        &[
            "credential",
            "create",
            "outside",
            "--out",
            path.to_str().ok_or("credential path is not UTF-8")?,
        ],
        None,
    )?;
    assert!(issued.get("secret").is_none());
    Ok(path)
}

fn exchange(proxy: &mut Process) -> TestResult<Vec<Value>> {
    let input = proxy.0.stdin.take().ok_or("proxy stdin is not piped")?;
    let output = proxy.0.stdout.take().ok_or("proxy stdout is not piped")?;
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let lines = BufReader::new(output)
            .lines()
            .map(|line| Ok(serde_json::from_str::<Value>(&line?)?))
            .collect::<TestResult<Vec<_>>>();
        let _ = tx.send(lines);
    });
    let frames = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","clientInfo":{},"capabilities":{}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"comb_get","arguments":{"slug":"components/writer"}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"comb_capture","arguments":{"text":"Investigated commit stalls","request_id":"capture-once"}}}),
    ];
    let mut input = input;
    for frame in frames {
        writeln!(input, "{frame}")?;
    }
    drop(input);
    let replies = rx.recv_timeout(Duration::from_secs(10))??;
    reader.join().map_err(|_| "MCP reader panicked")?;
    assert!(proxy.0.wait()?.success());
    Ok(replies)
}

fn verify_capture(root: &Path, socket: &Path, replies: &[Value]) -> TestResult {
    assert_eq!(replies.len(), 4);
    let document: Value = serde_json::from_str(
        replies[2]["result"]["content"][0]["text"]
            .as_str()
            .ok_or("reply text is missing")?,
    )?;
    assert_eq!(document["page"]["compiled"], "Durable commits.");
    let capture: Value = serde_json::from_str(
        replies[3]["result"]["content"][0]["text"]
            .as_str()
            .ok_or("reply text is missing")?,
    )?;
    let note = cli(
        root,
        socket,
        &[
            "get",
            capture["slug"].as_str().ok_or("reply text is missing")?,
        ],
        None,
    )?;
    assert_eq!(note["page"]["metadata"]["author"]["kind"], "client");
    assert_eq!(note["unverified"], true);
    Ok(())
}

#[test]
fn standalone_server_drains_structured_log_on_termination() -> TestResult {
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")?;
    let root = directory.path();
    let socket = root.join("comb.sock");
    cli(root, &socket, &["init"], None)?;
    let mut process = serve(root, &socket)?;
    let pid = rustix::process::Pid::from_raw(i32::try_from(process.0.id())?).ok_or("pid")?;
    rustix::process::kill_process(pid, rustix::process::Signal::TERM)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = process.0.try_wait()? {
            break status;
        }
        assert!(Instant::now() < deadline, "Server did not stop");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success());
    let path = root.join(".comb/logs/comb.jsonl");
    let records: Vec<Value> = std::fs::read_to_string(&path)?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    for name in ["comb.starting", "comb.ready", "comb.stopped"] {
        assert!(records.iter().any(|r| r["event"] == name));
    }
    assert_eq!(std::fs::metadata(path)?.permissions().mode() & 0o777, 0o600);
    assert!(comb::Comb::open(root).is_ok());
    Ok(())
}

#[test]
fn unsafe_diagnostic_path_is_a_visible_startup_failure_before_readiness() -> TestResult {
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")?;
    let root = directory.path();
    let socket = root.join("comb.sock");
    cli(root, &socket, &["init"], None)?;
    std::fs::write(root.join(".comb/logs"), "block log directory")?;
    let output = command(root, Some(&socket)).arg("serve").output()?;
    assert_eq!(output.status.code(), Some(1));
    let reply: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(reply["error"]["code"], "unsafe_path");
    assert!(!socket.exists());
    assert!(comb::Comb::open(root).is_ok());
    Ok(())
}

fn assert_socket_receipt(socket: &Path) -> TestResult {
    use sha2::{Digest, Sha256};
    let socket = socket.canonicalize()?;
    let hash = format!(
        "{:x}",
        Sha256::digest(socket.as_os_str().as_encoded_bytes())
    );
    let receipt: Value = serde_json::from_slice(&std::fs::read(
        socket.with_file_name(format!(".comb-socket-{hash}.json")),
    )?)?;
    let stat = std::fs::symlink_metadata(&socket)?;
    assert_eq!(receipt["entry"]["ctime"], stat.ctime());
    assert_eq!(receipt["entry"]["ctime_nsec"], stat.ctime_nsec());
    assert_eq!(receipt["entry"]["node"]["inode"], stat.ino());
    Ok(())
}
