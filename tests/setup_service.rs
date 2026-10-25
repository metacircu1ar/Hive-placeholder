#![allow(
    clippy::disallowed_methods,
    reason = "Raw writes are gated by the exact the reviewed maintainer write inventory inventory"
)]
//! Managed execution without a real service manager; all homes and units are isolated.
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod support;
use hive_setup_support::{
    files, managed,
    service::{Layout, Platform, Scope, Spec},
};
use serde_json::json;
use std::{fs, path::PathBuf, process::Command, time::Duration};

struct Fixture {
    home: tempfile::TempDir,
    config: PathBuf,
    state: PathBuf,
    spec: Spec,
}
impl Fixture {
    fn new(config: &str, environment: &str) -> Self {
        // Hive's launcher intentionally refuses HOME below world-writable /tmp.
        let base =
            PathBuf::from(std::env::var_os("HOME").unwrap()).join(".cache/hive-service-tests");
        files::directory(&base).unwrap();
        let home = tempfile::Builder::new()
            .prefix("svc-")
            .tempdir_in(base)
            .unwrap();
        let root = home.path().canonicalize().unwrap();
        let config_path = root.join("config.yaml");
        fs::write(&config_path, config).unwrap();
        let state = root.join("state");
        let layout = Layout {
            home: root.clone(),
            config: root.join("cfg"),
            state: root.join("st"),
            platform: Platform::current(),
        };
        let env_file = root.join("service.env");
        files::write(&env_file, environment.as_bytes(), false).unwrap();
        let scope = Scope::new("hive", &[&config_path, &state]).unwrap();
        let spec = Spec::new(
            &layout,
            scope,
            std::path::Path::new(env!("CARGO_BIN_EXE_hivectl")),
            vec!["daemon".into(), "run".into()],
            Some(&env_file),
            vec!["HTTP_PROXY".into()],
        )
        .unwrap();
        files::write(&spec.descriptor, &files::encode(&spec).unwrap(), false).unwrap();
        files::write(&spec.unit, &spec.render().unwrap(), false).unwrap();
        Self {
            home,
            config: config_path,
            state,
            spec,
        }
    }
    fn service_path(&mut self) {
        self.spec.environment.insert(
            "PATH".into(),
            format!("{}:/usr/bin:/bin", self.home.path().display()),
        );
        self.spec = self.spec.rendered().unwrap();
        files::write(
            &self.spec.descriptor,
            &files::encode(&self.spec).unwrap(),
            true,
        )
        .unwrap();
        files::write(&self.spec.unit, &self.spec.render().unwrap(), true).unwrap();
    }
    fn command(&self) -> Command {
        let mut cmd = support::command(env!("CARGO_BIN_EXE_hivectl"));
        cmd.env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.home.path().join("cfg"))
            .env("XDG_STATE_HOME", self.home.path().join("st"))
            .env("PATH", &self.spec.environment["PATH"])
            .env_remove("HTTP_PROXY")
            .arg("--config")
            .arg(&self.config)
            .arg("--state-dir")
            .arg(&self.state)
            .arg("--managed-service")
            .arg(&self.spec.descriptor);
        cmd
    }
    fn output(&self) -> String {
        format!(
            "stdout={}; stderr={}",
            fs::read_to_string(self.home.path().join("stdout")).unwrap(),
            fs::read_to_string(self.home.path().join("stderr")).unwrap()
        )
    }
}
#[test]
fn malformed_config_and_missing_declared_environment_stop_managed_retries() {
    for (config, environment, code) in [
        (
            "unknown_key: true",
            "HTTP_PROXY=http://example.invalid:9",
            "config_invalid",
        ),
        ("auth_profiles: {}", "OTHER=value", "config_invalid"),
    ] {
        let f = Fixture::new(config, environment);
        let output = f.command().args(["daemon", "run"]).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(managed::failure(&f.spec).unwrap().unwrap().code, code);
        assert!(String::from_utf8(output.stdout).unwrap().contains("error"));
    }
}
#[test]
fn ordinary_cli_config_failure_keeps_its_nonzero_exit() {
    let f = Fixture::new("unknown_key: true", "HTTP_PROXY=http://example.invalid:9");
    let out = support::command(env!("CARGO_BIN_EXE_hivectl"))
        .arg("--config")
        .arg(&f.config)
        .arg("--state-dir")
        .arg(&f.state)
        .args(["daemon", "run"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(managed::failure(&f.spec).unwrap().is_none());
}
#[test]
fn explicit_environment_reaches_provider_but_never_the_service_unit() {
    let sentinel = "http://private-service-proxy.invalid:9";
    let mut f = Fixture::new(
        "auth_profiles:\n  main: {provider: codex, method: subscription}\nroles:\n  worker: {auth: main, model: gpt-5.6-terra, instructions: role.md}\n",
        &format!("HTTP_PROXY={sentinel}"),
    );
    f.service_path();
    fs::write(f.home.path().join("role.md"), "test").unwrap();
    let profile = hive::config::Profile {
        provider: hive::config::Provider::Codex,
        method: hive::config::Method::Subscription,
        api_key_env: None,
        api_key_file: None,
    };
    hive::state::State::new(f.state.clone())
        .unwrap()
        .register("main", &profile)
        .unwrap();
    let workspace = f.home.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let received = f.home.path().join("received");
    let mock = f.home.path().join("codex");
    write_mock(&f, &mock, &received);
    let mut child = support::ChildGuard::new(
        f.command()
            .arg("--codex-bin")
            .arg(mock)
            .args(["daemon", "run"])
            .stdout(fs::File::create(f.home.path().join("stdout")).unwrap())
            .stderr(fs::File::create(f.home.path().join("stderr")).unwrap())
            .spawn()
            .unwrap(),
    );
    support::wait::wait_until(
        Duration::from_secs(12),
        Duration::from_millis(50),
        "managed service observation",
        || {
            assert!(child.try_wait().unwrap().is_none(), "{}", f.output());
            hive::runtime::client(&f.state, "ready", json!({"op":"ping"}))
                .ok()
                .filter(|v| v["status"] == "running")
                .map(|_| ())
        },
    );
    let reply = hive::runtime::client(
        &f.state,
        "spawn",
        json!({"op":"spawn","role":"worker","cwd":workspace,"body":"check environment"}),
    )
    .unwrap();
    if reply.get("error").is_some() {
        let _ = support::wait::try_wait_until(
            Duration::from_secs(5),
            Duration::from_millis(50),
            || child.try_wait().unwrap(),
        );
    }
    assert!(reply.get("error").is_none(), "{reply}; {}", f.output());
    support::wait::wait_until(
        Duration::from_secs(15),
        Duration::from_millis(50),
        "managed service observation",
        || received.exists().then_some(()),
    );
    assert_eq!(fs::read_to_string(received).unwrap(), sentinel);
    assert!(!fs::read_to_string(&f.spec.unit).unwrap().contains(sentinel));
    hive::runtime::client(&f.state, "stop", json!({"op":"shutdown"})).unwrap();
    support::wait::wait_until(
        Duration::from_secs(12),
        Duration::from_millis(50),
        "managed service observation",
        || child.try_wait().unwrap().is_some().then_some(()),
    );
    assert!(managed::failure(&f.spec).unwrap().is_none());
}

fn write_mock(f: &Fixture, mock: &std::path::Path, received: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let native = support::python_shebang(f.home.path());
    let interpreter = native.trim().strip_prefix("#!").unwrap();
    std::os::unix::fs::symlink(interpreter, f.home.path().join("service-python")).unwrap();
    let shebang = "#!/usr/bin/env service-python\n";
    fs::write(
        f.home.path().join("mock_common.py"),
        include_str!("fixtures/mock_common.py"),
    )
    .unwrap();
    support::mock_files(f.home.path());
    fs::write(mock, format!("{shebang}import os, pathlib\npathlib.Path({}).write_text(os.environ.get('HTTP_PROXY', 'missing'))\n{}", json!(received), include_str!("fixtures/runtime_provider.py"))).unwrap();
    fs::set_permissions(mock, fs::Permissions::from_mode(0o700)).unwrap();
}
