#![allow(
    clippy::disallowed_methods,
    reason = "Raw writes are gated by the exact the reviewed maintainer write inventory inventory"
)]
use comb::service::{Options, Server};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
    thread::JoinHandle,
};
use tokio::sync::oneshot;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const SLUG: &str = "concepts/removal";
const REQUEST_ID: &str = "remove-explicit";
const PAGE: &str = "---\ntitle: Removal\ntype: concept\n---\nReceipt-backed deletion.\n";

fn cli(root: &Path, args: &[&str], input: Option<&str>, success: bool) -> TestResult<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_comb"))
        .arg("--root")
        .arg(root)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(input) = input {
        child
            .stdin
            .take()
            .ok_or("CLI stdin was not piped")?
            .write_all(input.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    assert_eq!(
        output.status.success(),
        success,
        "{args:?}\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn field<'a>(reply: &'a Value, name: &str) -> TestResult<&'a str> {
    reply[name]
        .as_str()
        .ok_or_else(|| format!("Expected string field {name}: {reply}").into())
}

fn put(root: &Path, slug: &str, request_id: &str) -> TestResult<String> {
    let reply = cli(
        root,
        &["put", slug, "--request-id", request_id],
        Some(PAGE),
        true,
    )?;
    Ok(field(&reply, "hash")?.to_owned())
}

fn remove(
    root: &Path,
    slug: &str,
    hash: &str,
    request_id: Option<&str>,
    success: bool,
) -> TestResult<Value> {
    let mut args = vec!["rm", slug, "--expected-hash", hash];
    if let Some(request_id) = request_id {
        args.extend(["--request-id", request_id]);
    }
    cli(root, &args, None, success)
}

fn assert_removed(reply: &Value, slug: &str, request_id: &str, replayed: bool) {
    assert_eq!(
        reply,
        &json!({"removed": slug, "request_id": request_id, "replayed": replayed})
    );
}

fn removal_contract(root: &Path) -> TestResult<String> {
    let hash = put(root, SLUG, "create-removal")?;
    let other_slug = "concepts/other";
    let other_hash = put(root, other_slug, "create-other")?;
    let other_path = root.join("concepts/other.md");
    let other_bytes = std::fs::read(&other_path)?;

    let removed = remove(root, SLUG, &hash, Some(REQUEST_ID), true)?;
    assert_removed(&removed, SLUG, REQUEST_ID, false);
    assert!(!root.join("concepts/removal.md").exists());
    let replay = remove(root, SLUG, &hash, Some(REQUEST_ID), true)?;
    assert_removed(&replay, SLUG, REQUEST_ID, true);

    let changed_hash = "0".repeat(64);
    assert_ne!(hash, changed_hash);
    for (slug, expected_hash) in [(SLUG, changed_hash.as_str()), (other_slug, hash.as_str())] {
        let conflict = remove(root, slug, expected_hash, Some(REQUEST_ID), false)?;
        assert_eq!(conflict["error"]["code"], "request_conflict");
        assert_eq!(conflict["error"]["request_id"], REQUEST_ID);
        assert_eq!(conflict["error"]["file_committed"], false);
        assert_eq!(std::fs::read(&other_path)?, other_bytes);
        assert!(!root.join("concepts/removal.md").exists());
    }

    for invalid_id in [String::new(), "x".repeat(129)] {
        let invalid = remove(root, other_slug, &other_hash, Some(&invalid_id), false)?;
        assert_eq!(invalid["error"]["code"], "invalid_request_id");
        assert_eq!(invalid["error"]["field"], "--request-id");
        assert!(
            invalid["error"]["examples"]
                .as_array()
                .is_some_and(|v| !v.is_empty())
        );
        assert!(invalid["error"].get("request_id").is_none());
        assert!(invalid["error"].get("file_committed").is_none());
        assert_eq!(std::fs::read(&other_path)?, other_bytes);
    }

    let generated = remove(root, other_slug, &other_hash, None, true)?;
    let generated_id = field(&generated, "request_id")?;
    uuid::Uuid::parse_str(generated_id)?;
    assert_removed(&generated, other_slug, generated_id, false);
    let replay = remove(root, other_slug, &other_hash, Some(generated_id), true)?;
    assert_removed(&replay, other_slug, generated_id, true);
    assert!(!other_path.exists());
    assert_eq!(cli(root, &["stats"], None, true)?["pages"], 0);
    Ok(hash)
}

struct Service {
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<comb::Result<()>>>,
}

impl Service {
    fn start(root: &Path) -> TestResult<Self> {
        // Bind the default socket before spawning: CLI connections can queue
        // immediately, and the held writer lock prevents offline fallback.
        let server = Server::open(root, Options::default())?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        let (shutdown, receiver) = oneshot::channel();
        let thread = std::thread::spawn(move || {
            runtime.block_on(server.run(async {
                let _ = receiver.await;
            }))
        });
        Ok(Self {
            shutdown: Some(shutdown),
            thread: Some(thread),
        })
    }

    fn signal_shutdown(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }

    fn stop(mut self) -> TestResult {
        self.signal_shutdown();
        if let Some(thread) = self.thread.take() {
            let result = match thread.join() {
                Ok(result) => result,
                Err(panic) => std::panic::resume_unwind(panic),
            };
            result?;
        }
        Ok(())
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.signal_shutdown();
        if let Some(thread) = self.thread.take() {
            match thread.join() {
                Ok(Ok(())) => (),
                Ok(Err(error)) if std::thread::panicking() => {
                    eprintln!("fixture service cleanup also failed: {error}");
                }
                Ok(Err(error)) => panic!("fixture service cleanup failed: {error}"),
                Err(_) if std::thread::panicking() => {
                    eprintln!("fixture service also panicked during cleanup");
                }
                Err(payload) => std::panic::resume_unwind(payload),
            }
        }
    }
}

#[test]
fn offline_cli_removal_returns_durable_receipts_and_validates_ids() -> TestResult {
    let directory = tempfile::tempdir_in("/tmp")?;
    cli(directory.path(), &["init"], None, true)?;
    removal_contract(directory.path())?;
    assert!(!directory.path().join(".comb/service.sock").exists());
    Ok(())
}

#[test]
fn service_cli_removal_receipts_replay_offline_and_after_restart() -> TestResult {
    let directory = tempfile::tempdir_in("/tmp")?;
    let root = directory.path();
    cli(root, &["init"], None, true)?;
    let service = Service::start(root)?;
    let hash = removal_contract(root)?;
    service.stop()?;
    assert!(!root.join(".comb/service.sock").exists());

    let offline = remove(root, SLUG, &hash, Some(REQUEST_ID), true)?;
    assert_removed(&offline, SLUG, REQUEST_ID, true);
    let service = Service::start(root)?;
    let restarted = remove(root, SLUG, &hash, Some(REQUEST_ID), true)?;
    assert_removed(&restarted, SLUG, REQUEST_ID, true);
    service.stop()?;
    assert!(!root.join(".comb/service.sock").exists());
    Ok(())
}
