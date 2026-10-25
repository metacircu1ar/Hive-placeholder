use super::*;
use std::{
    io::{self, BufRead, Read},
    os::fd::OwnedFd,
    time::{Duration, Instant},
};

fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_comb"));
    command.arg("--root").arg(root).env_remove("COMB_LOG");
    command
}

#[test]
fn service_logging_options_are_install_only_and_validated_before_effects() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("missing");
    for args in [
        vec!["service", "start", "--log-level", "info"],
        vec!["service", "--log-files", "5", "status"],
        vec!["service", "install", "--log-file-mib", "0"],
        vec!["service", "install", "--log-files", "65"],
    ] {
        let output = command(&root).args(&args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(!root.exists());
    }
    let help = command(&root)
        .args(["service", "install", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    let text = String::from_utf8(help.stdout).unwrap();
    for option in ["--log-level", "--log-file-mib", "--log-files"] {
        assert!(text.contains(option));
    }
}

#[test]
fn logs_filter_without_opening_the_writer_and_never_create_missing_roots() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("knowledge");
    let _owner = comb::Comb::init(&root).unwrap();
    let path = root.join(".comb/logs/comb.jsonl");
    hive_setup_support::files::write(&path,
        b"{\"t\":1,\"level\":\"info\",\"event\":\"comb.ready\"}\n{\"t\":2,\"level\":\"warn\",\"event\":\"comb.retry\"}\n", false).unwrap();
    let output = command(&root)
        .args(["logs", "--level", "warn", "--tail", "1"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let record: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(record["event"], "comb.retry");
    let output = command(&root)
        .args(["logs", "--event", "comb.ready"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["event"],
        "comb.ready"
    );
    let missing = dir.path().join("missing");
    assert!(
        !command(&missing)
            .arg("logs")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(!missing.exists());
}

struct Running(std::process::Child);
impl Drop for Running {
    fn drop(&mut self) {
        let killed = self.0.kill();
        let waited = self.0.wait();
        if std::thread::panicking() {
            if let Err(error) = waited {
                eprintln!("Fixture cleanup could not reap child: {error}");
            }
        } else {
            assert!(
                killed.is_ok()
                    || killed
                        .as_ref()
                        .is_err_and(|e| e.kind() == io::ErrorKind::InvalidInput),
                "Cannot stop fixture: {killed:?}"
            );
            assert!(waited.is_ok(), "Cannot reap fixture: {waited:?}");
        }
    }
}

#[test]
fn follow_delivers_new_complete_records_without_opening_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let _owner = comb::Comb::init(dir.path()).unwrap();
    let path = dir.path().join(".comb/logs/comb.jsonl");
    hive_setup_support::files::write(
        &path,
        b"{\"t\":1,\"level\":\"info\",\"event\":\"comb.ready\"}\n",
        false,
    )
    .unwrap();
    let mut child = Running(
        command(dir.path())
            .args(["logs", "--follow"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let stdout = child.0.stdout.take().unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in io::BufReader::new(stdout).lines() {
            if send.send(line).is_err() {
                break;
            }
        }
    });
    assert!(
        receive
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap()
            .contains("comb.ready")
    );
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(b"{\"t\":2,\"level\":\"info\",\"event\":\"comb.stopped\"}\n")
        .unwrap();
    assert!(
        receive
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap()
            .contains("comb.stopped")
    );
    drop(child);
    reader.join().unwrap();
}

#[test]
fn blocked_diagnostic_stderr_does_not_hold_process_exit_or_json_stdout() {
    let (mut writer, _unread) = std::os::unix::net::UnixStream::pair().unwrap();
    writer.set_nonblocking(true).unwrap();
    loop {
        match writer.write(&[b'x'; 8192]) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) => panic!("{error}"),
        }
    }
    writer.set_nonblocking(false).unwrap();
    let mut child = Running(
        Command::new(env!("CARGO_BIN_EXE_comb"))
            .env("COMB_LOG", "private-invalid-log-level")
            .args(["version", "--json"])
            .stdout(Stdio::piped())
            .stderr(Stdio::from(OwnedFd::from(writer)))
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(
            Instant::now() < deadline,
            "blocked diagnostics prevented process exit"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut text = String::new();
    child
        .0
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap()["product"],
        "Comb"
    );
}

#[test]
fn invalid_serve_level_cannot_create_logging_or_knowledge_state() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("absent");
    let output = command(&root)
        .env("COMB_LOG", "private-invalid-log-level")
        .arg("serve")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!root.exists());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("private-invalid-log-level"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private-invalid-log-level"));
}
