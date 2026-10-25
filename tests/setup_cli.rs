#![allow(
    clippy::disallowed_methods,
    reason = "Raw writes are gated by the exact the reviewed maintainer write inventory inventory"
)]
//! The wizard drives real product commands against deterministic native providers.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "setup_cli/inputs.rs"]
mod inputs;
mod support;
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};
struct Fixture {
    home: tempfile::TempDir,
    config: PathBuf,
    state: PathBuf,
    codex: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let base = PathBuf::from(std::env::var_os("HOME").unwrap()).join(".cache/lx-setup");
        hive_setup_support::files::directory(&base).unwrap();
        let home = tempfile::Builder::new()
            .prefix("t")
            .tempdir_in(base)
            .unwrap();
        let root = home.path();
        let scenario = root.join("scenario.json");
        fs::write(&scenario, "{}").unwrap();
        fs::write(
            root.join("mock_common.py"),
            include_str!("fixtures/mock_common.py"),
        )
        .unwrap();
        support::mock_files(root);
        let codex = root.join("codex");
        fs::write(
            &codex,
            format!(
                "{}SCENARIO = {}\nPROVIDER = 'codex'\n{}",
                support::python_shebang(root),
                json!(scenario),
                include_str!("fixtures/provider.py")
            ),
        )
        .unwrap();
        fs::set_permissions(&codex, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            config: root.join("config.yaml"),
            state: root.join("state"),
            home,
            codex,
        }
    }
    fn product(&self, scope: &str) -> Command {
        let mut c = support::command(env!("CARGO_BIN_EXE_hivectl"));
        self.environment(&mut c);
        c.arg("--state-dir").arg(&self.state);
        if scope != "remote" {
            c.arg("--config").arg(&self.config);
        }
        if scope == "provider" {
            c.arg("--codex-bin").arg(&self.codex);
        }
        c
    }
    fn command(&self) -> Command {
        let mut c = self.product("provider");
        c.arg("setup");
        c
    }
    fn removal(&self) -> Command {
        let mut c = support::command(env!("CARGO_BIN_EXE_hivectl"));
        self.environment(&mut c);
        c.args(["setup", "--uninstall", "--yes", "--prefix"])
            .arg(self.home.path().join("installation"));
        c
    }
    fn start(&self) -> support::ChildGuard {
        use std::{
            process::Stdio,
            time::{Duration, Instant},
        };
        let mut daemon = support::ChildGuard::new(
            self.product("provider")
                .args(["daemon", "run"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let (out, _) = output(self.product("control").args(["daemon", "status"]));
            if out.status.success() {
                return daemon;
            }
            assert!(daemon.try_wait().unwrap().is_none());
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(30));
        }
    }
    fn environment(&self, c: &mut Command) {
        c.env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.home.path().join("cfg"))
            .env("XDG_STATE_HOME", self.home.path().join("st"))
            .env("XDG_DATA_HOME", self.home.path().join("data"));
    }
    fn pty(&self, args: &[&str], answers: Value) -> Output {
        let base = self.command();
        let mut c = support::command(support::python());
        self.environment(&mut c);
        c.env("HIVE_SETUP_ANSWERS", answers.to_string())
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pty_setup.py"))
            .arg(base.get_program())
            .args(base.get_args())
            .args(args)
            .output()
            .unwrap()
    }
}
fn output(c: &mut Command) -> (Output, Value) {
    let out = c.output().unwrap();
    let value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| {
        panic!(
            "{} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (out, value)
}
#[test]
fn uninstall_scope_requires_purge_before_executing_helper() {
    let f = Fixture::new();
    let prefix = f.home.path().join("installation");
    hive_setup_support::files::directory(&prefix.join("bin")).unwrap();
    atomic_fs::Publisher::new(
        (hive_setup_support::names::PUBLICATION_OPTIONS)
            .with_mode(0o700)
            .with_durability(atomic_fs::Durability::AtomicVisible),
    )
    .create_new(prefix.join("bin/hive-install"), b"#!/bin/sh\nexit 91\n")
    .unwrap();
    assert_eq!(f.removal().output().unwrap().status.code(), Some(91));
    for flag in [
        "--comb-root",
        "--bridge-file",
        "--config",
        "--state-dir",
        "--purge-comb-root",
    ] {
        let (out, value) = output(f.removal().arg(flag).arg(f.home.path().join("scope")));
        assert_eq!(out.status.code(), Some(2), "{value}");
        assert_eq!(value["error"]["code"], "conflicting_arguments");
        assert_eq!(value["error"]["field"], flag);
        assert!(
            value["error"]["expected"]
                .as_str()
                .unwrap()
                .contains("--purge")
        );
        assert!(String::from_utf8_lossy(&out.stderr).contains("Examples:"));
        let allowed = f
            .removal()
            .arg("--purge")
            .arg(flag)
            .arg(f.home.path().join("scope"))
            .output()
            .unwrap();
        assert_eq!(allowed.status.code(), Some(91));
    }
    assert!(!f.config.exists());
    assert!(!f.state.exists());
}

#[test]
fn uninstall_rejects_supplied_wizard_values_and_globals_before_helper() {
    let f = Fixture::new();
    for (flag, value) in [
        ("--provider", "codex"),
        ("--default-provider", "claude"),
        ("--login", "skip"),
        ("--codex-profile", "codex-main"),
        ("--claude-profile", "claude-main"),
        ("--codex-model", "model"),
        ("--claude-model", "sonnet"),
        ("--codex-effort", "low"),
        ("--claude-effort", "high"),
        ("--embedding-config", "missing"),
        ("--comb-bin", "missing"),
        ("--no-comb", "false"),
        ("--no-comb-roles", "false"),
        ("--collectors", "false"),
        ("--user-skills", "codex"),
        ("--skills-dir", "missing"),
        ("--install-provider", "claude"),
        ("--services", "false"),
        ("--no-services", "false"),
        ("--restart-daemon", "false"),
        ("--preserve-stopped-services", "false"),
        ("--service-environment", "missing"),
        ("--service-path", "/bin"),
        ("--codex-bin", "codex"),
        ("--claude-bin", "claude"),
        ("--probe-timeout", "15"),
        ("--login-timeout", "600"),
    ] {
        let (out, value) = output(f.removal().arg(format!("{flag}={value}")));
        assert_eq!(out.status.code(), Some(2), "{flag}: {value}");
        assert_eq!(value["error"]["code"], "conflicting_arguments");
        assert_eq!(value["error"]["field"], flag);
    }
    for flag in ["--config", "--state-dir"] {
        let (out, value) = output(f.removal().arg(flag).arg("missing"));
        assert_eq!(out.status.code(), Some(2), "{value}");
        assert_eq!(value["error"]["field"], flag);
        assert!(
            value["error"]["expected"]
                .as_str()
                .unwrap()
                .contains("--purge")
        );
    }
    assert!(!f.home.path().join("installation").exists());
    assert!(!f.config.exists());
    assert!(!f.state.exists());
}

#[test]
fn uninstall_answers_preserve_false_presence_and_supplied_service_flag() {
    let f = Fixture::new();
    let answers = f.home.path().join("removal.yaml");
    for flag in [
        "services",
        "no-services",
        "restart-daemon",
        "preserve-stopped-services",
    ] {
        hive_setup_support::files::write(
            &answers,
            format!("uninstall: true\n{flag}: false\n").as_bytes(),
            true,
        )
        .unwrap();
        let mut command = support::command(env!("CARGO_BIN_EXE_hivectl"));
        f.environment(&mut command);
        let (out, value) = output(command.args(["setup", "--answers"]).arg(&answers));
        assert_eq!(out.status.code(), Some(2), "{value}");
        assert_eq!(value["error"]["field"], "--answers");
        assert!(value["error"]["expected"].as_str().unwrap().contains(flag));
    }
    hive_setup_support::files::write(&answers, b"uninstall: true\n", true).unwrap();
    let mut command = support::command(env!("CARGO_BIN_EXE_hivectl"));
    f.environment(&mut command);
    let (out, value) = output(
        command
            .args(["--probe-timeout", "15", "setup", "--answers"])
            .arg(&answers),
    );
    assert_eq!(out.status.code(), Some(2), "{value}");
    assert_eq!(value["error"]["field"], "--probe-timeout");
}
#[test]
fn unattended_skip_is_repeatable_and_dry_run_has_no_writes() {
    let f = Fixture::new();
    let args = [
        "--yes",
        "--provider",
        "codex",
        "--login",
        "skip",
        "--codex-model",
        "fixture-default",
        "--codex-effort",
        "high",
        "--no-comb",
        "--no-services",
    ];
    let (out, value) = output(f.command().args(args).arg("--dry-run"));
    assert!(out.status.success(), "{value}");
    assert!(!f.config.exists());
    assert!(!f.state.exists());
    assert!(!f.home.path().join("cfg").exists());
    let (out, first) = output(f.command().args(args));
    assert!(out.status.success(), "{first}");
    assert_eq!(first["ready"], false);
    assert_eq!(first["configured"], true);
    let bytes = fs::read(&f.config).unwrap();
    let (out, again) = output(f.command().args(args));
    assert!(out.status.success(), "{again}");
    assert_eq!(again["changed"], false);
    assert!(again["backup"].is_null());
    assert_eq!(fs::read(&f.config).unwrap(), bytes);
    let (out, conflict) = output(f.command().args([
        "--yes",
        "--login",
        "skip",
        "--provider",
        "codex",
        "--codex-model",
        "different",
        "--no-comb",
        "--no-services",
    ]));
    assert!(!out.status.success(), "{conflict}");
    assert_eq!(fs::read(&f.config).unwrap(), bytes);
}
#[test]
fn prompts_and_native_login_use_tty_without_consuming_installer_pipe() {
    let f = Fixture::new();
    let output = f.pty(
        &["--no-comb", "--no-services"],
        json!([
            ["(--provider)", "codex"],
            ["(--login)", "browser"],
            ["fake-login: enter confirmation", "confirm"]
        ]),
    );
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let config = hive::config::Config::load(&f.config).unwrap();
    let resolved = config
        .resolve_spawn(hive::config::SpawnOptions::default())
        .unwrap();
    assert_eq!(
        (resolved.model.as_str(), resolved.effort.as_str()),
        ("fixture-default", "high")
    );
    let (out, value) = output_json(f.command().args([
        "--yes",
        "--login",
        "browser",
        "--no-comb",
        "--no-services",
    ]));
    assert!(out.status.success(), "{value}");
    assert_eq!(value["changed"], false);
    assert_eq!(value["providers"][0]["verified"], true);
}
fn output_json(c: &mut Command) -> (Output, Value) {
    output(c)
}

#[test]
fn failed_native_login_keeps_auth_evidence_without_publishing_config() {
    let f = Fixture::new();
    fs::write(
        f.home.path().join("scenario.json"),
        json!({"login_failure":true}).to_string(),
    )
    .unwrap();
    let output = f.pty(
        &[
            "--provider",
            "codex",
            "--login",
            "browser",
            "--no-comb",
            "--no-services",
        ],
        json!([["fake-login: enter confirmation", "confirm"]]),
    );
    assert!(!output.status.success());
    assert!(!f.config.exists());
    assert!(f.state.join("index.json").exists());
    assert!(String::from_utf8_lossy(&output.stdout).contains("SETUP_PIPE_PRESERVED"));
}
#[test]
fn opted_in_skill_copies_have_their_documentation_and_refuse_user_edits() {
    let f = Fixture::new();
    let skills = Path::new(env!("CARGO_MANIFEST_DIR")).join("skills");
    let args = [
        "--yes",
        "--provider",
        "codex",
        "--login",
        "skip",
        "--codex-model",
        "fixture-default",
        "--codex-effort",
        "high",
        "--no-comb",
        "--no-services",
        "--user-skills",
        "codex",
        "--skills-dir",
        skills.to_str().unwrap(),
    ];
    let (out, value) = output(f.command().args(args));
    assert!(out.status.success(), "{value}");
    let guide = f.home.path().join(".codex/skills/hive/SKILL.md");
    assert!(
        fs::read_to_string(&guide)
            .unwrap()
            .contains("bundle/docs/cli-reference.md")
    );
    assert!(
        guide
            .parent()
            .unwrap()
            .join("bundle/docs/cli-reference.md")
            .is_file()
    );
    let (out, value) = output(f.command().args(args));
    assert!(out.status.success(), "{value}");
    let config = fs::read(&f.config).unwrap();
    fs::write(&guide, "user owned guide").unwrap();
    let (out, value) = output(f.command().args(args));
    assert!(!out.status.success(), "{value}");
    assert_eq!(fs::read(&f.config).unwrap(), config);
    assert_eq!(fs::read_to_string(guide).unwrap(), "user owned guide");
}

#[test]
fn identical_setup_keeps_pending_restart_and_never_stops_manual_daemon() {
    let f = Fixture::new();
    let args = [
        "--yes",
        "--provider",
        "codex",
        "--login",
        "skip",
        "--codex-model",
        "fixture-default",
        "--codex-effort",
        "high",
        "--no-comb",
        "--no-services",
    ];
    let (out, value) = output(f.command().args(args));
    assert!(out.status.success(), "{value}");
    let mut daemon = f.start();
    let mut config = fs::read_to_string(&f.config).unwrap();
    config.push_str("# pending configuration\n");
    fs::write(&f.config, config).unwrap();
    for restart in [false, false, true] {
        let mut command = f.command();
        command.args(args);
        if restart {
            command.arg("--restart-daemon");
        }
        let (out, result) = output(&mut command);
        assert!(out.status.success(), "{result}");
        assert_eq!(result["changed"], false);
        assert_eq!(result["restart_pending"], true);
        assert_eq!(result["ready"], false);
        assert!(daemon.try_wait().unwrap().is_none());
    }
    assert!(
        f.product("control")
            .args(["daemon", "stop"])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(daemon.wait().unwrap().success());
}

#[test]
fn catalog_defaults_reach_the_first_mock_agent_turn() {
    use hive::{
        config::{Method, Profile, Provider},
        state::State,
    };
    let f = Fixture::new();
    let shebang = fs::read_to_string(&f.codex)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_owned();
    fs::write(&f.codex,format!("{shebang}\nimport sys\nif '--version' in sys.argv:\n    print('codex-cli 0.154.0')\n    sys.exit(0)\n{}",include_str!("fixtures/runtime_provider.py"))).unwrap();
    let binding = State::new(f.state.clone())
        .unwrap()
        .register(
            "codex-main",
            &Profile {
                provider: Provider::Codex,
                method: Method::Subscription,
                api_key_env: None,
                api_key_file: None,
            },
        )
        .unwrap();
    let (out, result) = output(f.command().args([
        "--yes",
        "--provider",
        "codex",
        "--login",
        "skip",
        "--no-comb",
        "--no-services",
    ]));
    assert!(out.status.success(), "{result}");
    assert_eq!(result["providers"][0]["model"], "gpt-5.6-terra");
    assert_eq!(result["providers"][0]["effort"], "high");
    let mut daemon = f.start();
    let workspace = f.home.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let (out, spawn) = output(
        f.product("remote")
            .args(["spawn", "--cwd"])
            .arg(&workspace)
            .args(["--body", "complete"]),
    );
    assert!(out.status.success(), "{spawn}");
    let id = spawn["agent_id"].as_str().unwrap();
    let (out, done) =
        output(
            f.product("remote")
                .args(["wait", id, "--until", "completed", "--timeout", "15"]),
        );
    assert!(out.status.success(), "{done}");
    let turns =
        fs::read_to_string(binding.directory.join("provider/turn-selections.jsonl")).unwrap();
    let turn: Value = serde_json::from_str(turns.lines().next().unwrap()).unwrap();
    assert_eq!(turn["model"], "gpt-5.6-terra");
    assert_eq!(turn["effort"], "high");
    assert!(
        f.product("control")
            .args(["daemon", "stop"])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(daemon.wait().unwrap().success());
}

#[cfg(feature = "comb")]
#[test]
fn positive_comb_paths_without_installed_binary_fail_before_auth_or_config() {
    let f = Fixture::new();
    let binary = f.home.path().join("hivectl");
    fs::copy(env!("CARGO_BIN_EXE_hivectl"), &binary).unwrap();
    for flag in ["--comb-root", "--bridge-file"] {
        let mut c = support::command(&binary);
        f.environment(&mut c);
        let (out, value) = output(
            c.env("PATH", "/usr/bin:/bin")
                .arg("--config")
                .arg(&f.config)
                .arg("--state-dir")
                .arg(&f.state)
                .args([
                    "setup",
                    "--yes",
                    "--provider",
                    "codex",
                    "--login",
                    "skip",
                    "--codex-model",
                    "test",
                    "--codex-effort",
                    "low",
                    flag,
                ])
                .arg(f.home.path().join("assets")),
        );
        assert!(!out.status.success(), "{value}");
        assert!(
            value.to_string().contains("installed Comb binary"),
            "{value}"
        );
        assert!(!f.config.exists());
        assert!(!f.state.exists());
    }
}

#[test]
#[cfg(feature = "comb")]
fn pipe_embedding_config_fails_before_any_preview_or_provider_process() {
    use std::{
        io::Read as _,
        process::Stdio,
        time::{Duration, Instant},
    };
    let f = Fixture::new();
    let fifo = f.home.path().join("PRIVATE.fifo");
    let native = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: a live NUL-terminated path; private FIFO mode.
    assert_eq!(unsafe { libc::mkfifo(native.as_ptr(), 0o600) }, 0);
    let sentinel = f.home.path().join("unexpected-process");
    let executable = f.home.path().join("must-not-run");
    let shebang = format!("#!{}", f.home.path().join("python3").display());
    fs::write(
        &executable,
        format!(
            "{}\nfrom pathlib import Path\nPath({}).touch()\nraise SystemExit(99)\n",
            shebang,
            json!(sentinel)
        ),
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let mut child = support::ChildGuard::new(
        f.product("control")
            .arg("--codex-bin")
            .arg(&executable)
            .args([
                "setup",
                "--yes",
                "--provider",
                "codex",
                "--login",
                "skip",
                "--codex-model",
                "fixture",
                "--codex-effort",
                "low",
                "--no-services",
                "--comb-bin",
            ])
            .arg(&executable)
            .arg("--embedding-config")
            .arg(&fifo)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "Hive setup blocked opening FIFO");
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut text = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert_eq!(status.code(), Some(2), "{text}");
    let result: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(result["error"]["field"], "--embedding-config");
    assert!(!sentinel.exists());
    assert!(!f.config.exists());
    assert!(!f.state.exists());
}

#[test]
fn setup_relationship_errors_are_typed_before_configuration_or_helpers() {
    let f = Fixture::new();
    let cases: &[(&[&str], &str)] = &[
        (&["--purge"], "--purge"),
        (&["--prefix", "/missing"], "--prefix"),
        (&["--purge-comb-root", "/missing"], "--purge-comb-root"),
        (&["--services", "--no-services"], "--services"),
        (
            &[
                "--answers",
                "/missing-answers",
                "--services",
                "--no-services",
            ],
            "--services",
        ),
        (&["--no-comb", "--collectors"], "--collectors"),
        (
            &["--no-services", "--service-path", "/bin"],
            "--service-path",
        ),
        (
            &["--no-services", "--service-environment", "/missing"],
            "--service-environment",
        ),
    ];
    for &(args, field) in cases {
        let (out, value) = output(f.command().args(args));
        assert_eq!(out.status.code(), Some(2), "{args:?}: {value}");
        assert_eq!(value["error"]["code"], "conflicting_arguments");
        assert_eq!(value["error"]["field"], field);
        for key in ["message", "expected"] {
            assert!(
                value["error"][key]
                    .as_str()
                    .is_some_and(|text| !text.is_empty())
            );
        }
        assert!(
            value["error"]["examples"]
                .as_array()
                .is_some_and(|items| !items.is_empty())
        );
    }
    assert!(!f.config.exists());
    assert!(!f.state.exists());
}
