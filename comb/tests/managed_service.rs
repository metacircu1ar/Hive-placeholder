#![allow(
    clippy::disallowed_methods,
    reason = "Raw writes are gated by the exact the reviewed maintainer write inventory inventory"
)]
//! Real managed CLI entry, using private fixtures and no operating-system job.
#![allow(clippy::unwrap_used)]
use comb::{Comb, embedding::HttpConfig, provision};
#[path = "support/embedding_http.rs"]
mod embedding_http;
#[path = "managed_service/raw_retention.rs"]
mod raw_retention;
use hive_setup_support::{
    files, managed,
    service::{Layout, Platform, Scope, Spec},
};
use std::{
    path::PathBuf,
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
        if !std::thread::panicking() {
            assert!(
                cleanup.is_ok(),
                "managed fixture cleanup failed: {cleanup:?}"
            );
        }
    }
}
struct Fixture {
    _http: embedding_http::Server,
    _temp: tempfile::TempDir,
    root: PathBuf,
    layout: Layout,
    spec: Spec,
}
impl Fixture {
    fn new() -> Self {
        let http = embedding_http::Server::new();
        let temp = tempfile::tempdir_in("/tmp").unwrap();
        let home = temp.path().canonicalize().unwrap();
        let root = home.join("root");
        drop(Comb::init(&root).unwrap());
        provision::use_http(
            &root,
            HttpConfig {
                model: "fixture".into(),
                revision: "pinned".into(),
                url: http.url.clone(),
                dimensions: 2,
                query_prefix: String::new(),
                document_prefix: String::new(),
                api_key_env: None,
                timeout_seconds: 1,
                request_dimensions: false,
                tokenizer: None,
                max_tokens: None,
            },
        )
        .unwrap();
        let mut settings = provision::settings(&root).unwrap().unwrap();
        if let Some(comb::embedding::Config::Http(config)) = &mut settings.embeddings {
            config.api_key_env = Some("COMB_SERVICE_TEST_KEY".into());
        }
        files::write(
            &root.join(".comb/serve.json"),
            &files::encode(&settings).unwrap(),
            true,
        )
        .unwrap();
        let layout = Layout {
            home: home.clone(),
            config: home.join("cfg"),
            state: home.join("state"),
            platform: Platform::current(),
        };
        let environment = home.join("service.env");
        files::write(
            &environment,
            b"COMB_SERVICE_TEST_KEY=private-fixture-value\n",
            false,
        )
        .unwrap();
        let scope = Scope::new("comb", &[&root]).unwrap();
        let spec = Spec::new(
            &layout,
            scope,
            std::path::Path::new(env!("CARGO_BIN_EXE_comb")),
            vec!["serve".into()],
            Some(&environment),
            vec![],
        )
        .unwrap();
        files::write(&spec.descriptor, &files::encode(&spec).unwrap(), false).unwrap();
        files::write(&spec.unit, &spec.render().unwrap(), false).unwrap();
        Self {
            _http: http,
            _temp: temp,
            root,
            layout,
            spec,
        }
    }
    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_comb"));
        cmd.env("HOME", &self.layout.home)
            .env("XDG_CONFIG_HOME", &self.layout.config)
            .env("XDG_STATE_HOME", &self.layout.state)
            .env_remove("COMB_SERVICE_TEST_KEY")
            .arg("--root")
            .arg(&self.root)
            .arg("--managed-service")
            .arg(&self.spec.descriptor)
            .arg("serve");
        cmd
    }
}
#[test]
fn managed_http_uses_private_environment_and_sigterm_releases_writer_and_socket() {
    let f = Fixture::new();
    let mut child = Running(
        f.command()
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        assert!(child.0.try_wait().unwrap().is_none());
        assert!(Instant::now() < deadline);
        if let Some(health) = comb::services::health(&f.root, true).unwrap() {
            assert!(health.model.unwrap().query_ready);
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !std::fs::read_to_string(&f.spec.unit)
            .unwrap()
            .contains("private-fixture-value")
    );
    rustix::process::kill_process(
        rustix::process::Pid::from_child(&child.0),
        rustix::process::Signal::TERM,
    )
    .unwrap();
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(comb::services::health(&f.root, true).unwrap().is_none());
    drop(Comb::open(&f.root).unwrap());
    let log = std::fs::read_to_string(f.root.join(".comb/logs/comb.jsonl")).unwrap();
    let events: Vec<serde_json::Value> = log
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let signals: Vec<_> = events
        .iter()
        .filter(|v| v["event"] == "comb.shutdown_signal")
        .collect();
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0]["signal"], libc::SIGTERM);
    assert!(!log.contains("private-fixture-value"));
}

#[test]
fn managed_pre_logger_configuration_failure_retains_classified_evidence() {
    let f = Fixture::new();
    files::write(&f.root.join(".comb/serve.json"), b"{broken\n", true).unwrap();
    let output = f.command().output().unwrap();
    assert!(output.status.success());
    let failure = managed::failure(&f.spec).unwrap().unwrap();
    assert!(failure.suppressed);
    assert_eq!(failure.code, "invalid_embedding_config");
    assert!(!f.root.join(".comb/logs").exists());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("comb.service_failed"));
    assert!(!stderr.contains("private-fixture-value"));
}

#[test]
fn managed_pre_logger_busy_failure_is_recorded_without_suppressing_retry() {
    let f = Fixture::new();
    let _owner = Comb::open(&f.root).unwrap();
    let output = f.command().output().unwrap();
    assert!(!output.status.success());
    let failure = managed::failure(&f.spec).unwrap().unwrap();
    assert!(!failure.suppressed);
    assert_eq!(failure.code, "comb_busy");
    assert!(!f.root.join(".comb/logs").exists());
}

#[test]
fn unsafe_failure_record_is_rejected_before_managed_entry() {
    let f = Fixture::new();
    // A directory cannot be admitted or replaced as a current failure record.
    let pending = f.spec.descriptor.with_extension("failure.json");
    std::fs::create_dir(&pending).unwrap();
    let output = f.command().output().unwrap();
    assert!(!output.status.success());
    assert!(pending.is_dir());
    assert!(std::fs::read_dir(&pending).unwrap().next().is_none());
    assert!(!f.root.join(".comb/logs").exists());
}
#[test]
fn declared_http_key_and_malformed_environment_fail_without_manager_retries() {
    for contents in ["OTHER=value", "not an environment assignment"] {
        let f = Fixture::new();
        files::write(
            f.spec.environment_file.as_ref().unwrap(),
            contents.as_bytes(),
            true,
        )
        .unwrap();
        let output = f.command().output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(managed::failure(&f.spec).unwrap().is_some());
        assert!(String::from_utf8_lossy(&output.stdout).contains("error"));
    }
}

#[test]
fn pending_http_credentials_are_checked_before_recovery_and_preserved_on_failure() {
    let f = Fixture::new();
    let mut pending = provision::settings(&f.root).unwrap().unwrap();
    if let Some(comb::embedding::Config::Http(config)) = &mut pending.embeddings {
        config.api_key_env = Some("PENDING_HTTP_KEY".into());
    }
    let mut current = pending.clone();
    current.embeddings = None;
    files::write(
        &f.root.join(".comb/serve.json"),
        &files::encode(&current).unwrap(),
        true,
    )
    .unwrap();
    files::write(
        &f.root.join(".comb/model-transition.json"),
        &files::encode(&pending).unwrap(),
        false,
    )
    .unwrap();
    assert_eq!(
        comb::services::required_environment(&f.root).unwrap(),
        ["PENDING_HTTP_KEY"]
    );
    let output = f.command().output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        managed::failure(&f.spec).unwrap().unwrap().code,
        "invalid_embedding_config"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("PENDING_HTTP_KEY"));
    drop(Comb::open(&f.root).unwrap());
}

#[test]
fn service_restart_failure_preserves_operation_code_and_commit_flag() {
    for committed in [false, true] {
        let original = comb::Error::new(comb::ErrorCode::RecoveryRequired, "accepted journal");
        let original = if committed {
            original.committed()
        } else {
            original
        };
        let combined = std::io::Error::other(hive_setup_support::service::RestartFailure {
            operation: Some(std::io::Error::other(original)),
            restart: std::io::Error::other("bootstrap failed"),
        });
        let converted = comb::services::error(combined);
        assert_eq!(converted.code, comb::ErrorCode::RecoveryRequired);
        assert_eq!(converted.file_committed, committed);
        assert!(converted.message.contains("bootstrap failed"));
    }
    let converted = comb::services::error(std::io::Error::other(
        hive_setup_support::service::RestartFailure {
            operation: None,
            restart: std::io::Error::other("bootstrap failed"),
        },
    ));
    assert!(converted.file_committed);
}

#[test]
fn empty_probe_connection_does_not_break_embedding_fixture() {
    let server = embedding_http::Server::new();
    let url = reqwest::Url::parse(&server.url).unwrap();
    drop(std::net::TcpStream::connect((url.host_str().unwrap(), url.port().unwrap())).unwrap());
    let response: serde_json::Value = reqwest::blocking::Client::new()
        .post(&server.url)
        .json(&serde_json::json!({"input":["probe"]}))
        .send()
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(
        response["data"][0]["embedding"],
        serde_json::json!([1.0, 0.0])
    );
}

#[test]
fn reserved_socket_keeps_managed_failure_evidence_before_service_entry() {
    let f = Fixture::new();
    let failure = f.spec.descriptor.with_extension("failure.json");
    files::write(
        &failure,
        b"{\"code\":\"saved_fixture\",\"suppressed\":false}\n",
        false,
    )
    .unwrap();
    let before = std::fs::read(&failure).unwrap();
    let socket = f.layout.home.join(".hive-tmp-state-ABC123");
    let result = f.command().arg("--socket").arg(&socket).output().unwrap();
    assert_eq!(result.status.code(), Some(2));
    let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value["error"]["field"], "--socket");
    assert_eq!(value["error"]["code"], "invalid_path");
    assert_eq!(std::fs::read(&failure).unwrap(), before);
    assert!(!socket.exists());
}
