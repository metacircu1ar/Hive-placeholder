#![allow(
    clippy::disallowed_methods,
    reason = "Raw writes are gated by the exact the reviewed maintainer write inventory inventory"
)]
//! Setup default paths and stored socket routing through actual release binaries.
#![allow(clippy::unwrap_used)] // Test harness setup and decoded fixture assertions.
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
struct Running(Child);
impl Drop for Running {
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
fn command(home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_comb"));
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .current_dir(home);
    cmd
}
fn json(command: &mut Command) -> Value {
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}
#[test]
fn setup_defaults_to_visible_root_and_commands_inherit_its_custom_socket() {
    let temp = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .unwrap();
    let home = temp.path().canonicalize().unwrap();
    let socket = home.join("custom.sock");
    let preview = json(command(&home).arg("--socket").arg(&socket).args([
        "setup",
        "--no-services",
        "--for-hive",
        "--dry-run",
        "--yes",
    ]));
    assert_eq!(preview["plan"]["root"], home.join("Comb").to_str().unwrap());
    assert_eq!(std::fs::read_dir(&home).unwrap().count(), 0);
    let initial = json(command(&home).arg("--socket").arg(&socket).args([
        "setup",
        "--no-services",
        "--for-hive",
        "--yes",
    ]));
    assert_eq!(initial["changed"], true);
    let again = json(command(&home).args(["setup", "--no-services", "--for-hive", "--yes"]));
    assert_eq!(again["changed"], false);
    let root = home.join("Comb");
    let mut process = Running(
        command(&home)
            .arg("--root")
            .arg(&root)
            .arg("serve")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::os::unix::net::UnixStream::connect(&socket).is_err() {
        assert!(process.0.try_wait().unwrap().is_none());
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let stats = json(command(&home).arg("--root").arg(&root).arg("stats"));
    assert_eq!(stats["pages"], 0);
}

#[path = "support/embedding_http.rs"]
mod embedding_http;
#[test]
fn http_setup_selects_exact_configuration_and_reuses_it_without_endpoint() {
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let home = temp.path().canonicalize().unwrap();
    let server = embedding_http::Server::new();
    let config = home.join("embedding.json");
    std::fs::write(&config,serde_json::json!({"backend":"http","model":"fixture","revision":"exact","dimensions":2,"url":server.url,"timeout_seconds":1}).to_string()).unwrap();
    let initial = json(
        command(&home)
            .args(["setup", "--no-services", "--embedding-config"])
            .arg(&config),
    );
    assert_eq!(initial["changed"], true);
    assert_eq!(initial["plan"]["http"]["model"], "fixture");
    let root = home.join("Comb");
    let saved = std::fs::read(root.join(".comb/serve.json")).unwrap();
    drop(server);
    let again = json(
        command(&home)
            .args(["setup", "--no-services", "--embedding-config"])
            .arg(&config),
    );
    assert_eq!(again["changed"], false);
    assert_eq!(std::fs::read(root.join(".comb/serve.json")).unwrap(), saved);
    // A different unreachable candidate must retain the previous selected configuration.
    let mut candidate: Value = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
    candidate["revision"] = "different".into();
    std::fs::write(&config, candidate.to_string()).unwrap();
    let failed = command(&home)
        .args(["setup", "--no-services", "--embedding-config"])
        .arg(&config)
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert_eq!(std::fs::read(root.join(".comb/serve.json")).unwrap(), saved);
}

#[test]
fn setup_pipe_configuration_has_one_consumer_through_apply() {
    use std::io::{Read as _, Write as _};
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let server = embedding_http::Server::new();
    let bytes = serde_json::json!({"backend":"http","model":"pipe-fixture","revision":"once","dimensions":2,"url":server.url,"timeout_seconds":1}).to_string();
    let mut child = Running(
        command(dir.path())
            .args(["setup", "--no-services", "--embedding-config", "/dev/stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    child
        .0
        .stdin
        .take()
        .unwrap()
        .write_all(bytes.as_bytes())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "setup reread or blocked on consumed pipe"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut stdout = String::new();
    let mut stderr = String::new();
    child
        .0
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    child
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(status.success(), "{stdout}\n{stderr}");
    let result: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(result["plan"]["http"]["revision"], "once");
    assert_eq!(result["changed"], true);
    let saved: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("Comb/.comb/serve.json")).unwrap())
            .unwrap();
    assert_eq!(saved["embeddings"]["model"], "pipe-fixture");
}

#[test]
fn malformed_setup_config_is_reported_before_home_and_managed_state() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("input");
    std::fs::write(&config, b"PRIVATE: [").unwrap();
    let output = command(dir.path())
        .env_remove("HOME")
        .env("XDG_CONFIG_HOME", "invalid-relative")
        .args(["setup", "--embedding-config"])
        .arg(&config)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(reply["error"]["field"], "--embedding-config");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("PRIVATE"));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}
