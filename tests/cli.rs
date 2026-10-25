#![allow(
    clippy::disallowed_methods,
    reason = "Raw writes are gated by the exact the reviewed maintainer write inventory inventory"
)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // Integration fixture helpers panic on failure.

mod support;

#[path = "cli/resources.rs"]
mod resources;

#[path = "cli/input.rs"]
mod input;

#[path = "cli/files.rs"]
mod files;

#[path = "cli/approvals.rs"]
mod approvals;

#[path = "cli/ordering.rs"]
mod ordering;

#[path = "cli/outcomes.rs"]
mod outcomes;

#[path = "cli/auth_selection.rs"]
mod auth_selection;

#[path = "cli/api_keys.rs"]
mod api_keys;

#[path = "cli/logging.rs"]
mod logging;

use hive::{
    config::{Method, Profile, Provider},
    state::State,
};
use resources::{PROBE_POLL_INTERVAL, PROBE_READY_TIMEOUT, ProbeRelease};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{Duration, Instant},
};
use support::wait::wait_until;

const STATUS_DEADLINE: Duration = Duration::from_secs(12);

include!("cli/fixture.rs");

fn cli_binary() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_hivectl"))
}

#[test]
fn list_and_missing_status_have_no_side_effects() {
    let f = Fixture::new();
    let (out, list) = f.run(&["list"]);
    assert!(out.status.success());
    assert_eq!(list["profiles"].as_array().unwrap().len(), 3);
    assert!(!f.state.root.exists());
    let (out, status) = f.run(&["status", "codex-main"]);
    assert!(out.status.success());
    assert_eq!(status["state"], "missing");
    assert!(!f.state.root.exists());
    let (_, login) = f.run(&["login", "codex-main"]);
    assert_eq!(login["error"]["code"], "interactive_terminal_required");
    assert!(!f.state.root.exists());
}

#[test]
fn doctor_aggregates_native_status_without_exposing_api_keys_or_account_identity() {
    let f = Fixture::new();
    f.register(Provider::Codex);
    f.register(Provider::Claude);
    f.scenario(json!({"state":"authenticated"}));
    let mut command = f.command();
    let args: Vec<_> = command.get_args().map(std::ffi::OsStr::to_owned).collect();
    command = support::command(env!("CARGO_BIN_EXE_hivectl"));
    let out = command
        .args(&args[..args.len() - 1])
        .arg("doctor")
        .env("HIVE_CANARY", "doctor-private-canary")
        .output()
        .unwrap();
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(1)); // The fixture deliberately has no daemon.
    let checks = value["checks"].as_array().unwrap();
    for profile in ["codex-main", "claude-main"] {
        let check = checks
            .iter()
            .find(|c| c["name"] == format!("auth:{profile}"))
            .unwrap();
        assert_eq!(check["detail"]["state"], "authenticated", "{value}");
        assert!(check["detail"].get("account_identity").is_none());
    }
    for output in [&out.stdout, &out.stderr] {
        assert!(!String::from_utf8_lossy(output).contains("doctor-private-canary"));
    }
}

#[test]
fn version_output_formats_need_neither_home_nor_config() {
    for json in [false, true] {
        let mut command = support::command(env!("CARGO_BIN_EXE_hivectl"));
        command.arg("version");
        if json {
            command.arg("--json");
        }
        let out = command
            .env_remove("HOME")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_STATE_HOME")
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        assert!(out.stderr.is_empty());
        let mut features = if cfg!(feature = "comb") {
            vec!["comb"]
        } else {
            vec![]
        };
        if cfg!(feature = "barrier-delay") {
            features.push("barrier-delay");
        }
        if json {
            let value: Value = serde_json::from_slice(&out.stdout).unwrap();
            #[cfg(feature = "comb")]
            let shared = serde_json::json!({
                "setup":hive_setup_support::compatibility::FORMATS,
                "comb_protocol":comb_protocol::compatibility::FORMATS,
            });
            #[cfg(not(feature = "comb"))]
            let shared = serde_json::json!({
                "setup":hive_setup_support::compatibility::FORMATS,
            });
            assert_eq!(
                value,
                serde_json::json!({
                    "product":"Hive",
                    "product_version":env!("CARGO_PKG_VERSION"),
                    "commit":hive_setup_support::identity::COMMIT,
                    "compiled_features":features,
                    "formats":hive::compatibility::FORMATS,
                    "shared_formats":shared,
                    "mcp_protocols":{"claude":hive::compatibility::CLAUDE_MCP,"external":hive::compatibility::EXTERNAL_MCP},
                    "provider_compatibility":[
                        {"provider":"codex","minimum":"0.154.0","last_tested":"0.161.0","corpus_versions":["0.154.0","0.161.0"]},
                        {"provider":"claude","minimum":"2.1.285","last_tested":"2.1.285","corpus_versions":["2.1.285"]}
                    ],
                })
            );
        } else {
            let variant = match (cfg!(feature = "comb"), cfg!(feature = "barrier-delay")) {
                (true, false) => "comb",
                (true, true) => "comb, barrier-delay",
                (false, false) => "core",
                (false, true) => "core, barrier-delay",
            };
            assert_eq!(
                String::from_utf8(out.stdout).unwrap(),
                format!("Hive {} ({variant})\n", env!("CARGO_PKG_VERSION"))
            );
        }
    }
}

#[test]
fn version_output_failure_is_operational_for_both_formats() {
    use std::{
        os::{fd::OwnedFd, unix::net::UnixStream},
        process::Stdio,
    };
    for json in [false, true] {
        let (reader, writer) = UnixStream::pair().unwrap();
        drop(reader);
        let mut command = support::command(env!("CARGO_BIN_EXE_hivectl"));
        command
            .arg("version")
            .stdout(Stdio::from(OwnedFd::from(writer)));
        if json {
            command.arg("--json");
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn doctor_keeps_json_and_failure_exit_when_stderr_is_unwritable() {
    let temp = tempfile::tempdir().unwrap();
    let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
        .arg("--config")
        .arg(temp.path().join("missing.yaml"))
        .arg("--state-dir")
        .arg(temp.path().join("state"))
        .arg("doctor")
        .stderr(fs::File::open("/dev/null").unwrap())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["name"] == "config")
    );
}

#[test]
fn api_presence_never_leaks_or_claims_validation() {
    let f = Fixture::new();
    let canary = "private-canary-should-never-appear";
    let out = f
        .command()
        .args(["status", "api"])
        .env("HIVE_CANARY", canary)
        .output()
        .unwrap();
    let status: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(status["state"], "present_unverified");
    assert!(!String::from_utf8_lossy(&out.stdout).contains(canary));
    assert!(!f.state.root.exists());
    for key in [None, Some("")] {
        let mut cmd = f.command();
        cmd.args(["status", "api"]).env_remove("HIVE_CANARY");
        if let Some(value) = key {
            cmd.env("HIVE_CANARY", value);
        }
        let status: Value = serde_json::from_slice(&cmd.output().unwrap().stdout).unwrap();
        assert_eq!(status["state"], "missing");
    }
    let out = f
        .command()
        .args(["login", "api"])
        .env_remove("HIVE_CANARY")
        .output()
        .unwrap();
    let login: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(login["outcome"], "registered");
    assert_eq!(login["state"], "missing");
    let generation = login["generation"].clone();
    for _ in 0..2 {
        let out = f
            .command()
            .args(["login", "api"])
            .env("HIVE_CANARY", canary)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["generation"], generation);
        assert_eq!(report["state"], "present_unverified");
        assert!(!String::from_utf8_lossy(&out.stdout).contains(canary));
    }
    let generations = f.state.generations("api").unwrap();
    assert_eq!(generations.len(), 1);
    assert!(!generations[0].authenticated);
    assert!(generations[0].account_identity.is_none());
    assert!(generations[0].key_fingerprint.is_none());
    assert_eq!(generations[0].key_revision, 0);
    let bytes = fs::read(f.state.root.join("index.json")).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains(canary));
    assert!(
        !generations[0]
            .binding
            .directory
            .join("provider/auth.json")
            .exists()
    );
}

#[test]
fn both_providers_have_structured_status_and_conflicts() {
    let f = Fixture::new();
    f.register(Provider::Codex);
    f.register(Provider::Claude);
    for provider in ["codex-main", "claude-main"] {
        f.scenario(json!({"state":"authenticated"}));
        let (out, status) = f.run(&["status", provider]);
        assert!(out.status.success());
        assert_eq!(status["state"], "authenticated", "{status}");
        f.scenario(json!({"state":"api"}));
        let (_, status) = f.run(&["status", provider]);
        assert_eq!(status["state"], "method_conflict", "{status}");
        f.scenario(json!({"state":"malformed"}));
        let (_, status) = f.run(&["status", provider]);
        assert_eq!(status["state"], "unavailable");
        assert!(!status.to_string().contains("secret-canary"));
    }
}

#[test]
fn timeout_and_output_limits_are_bounded() {
    let f = Fixture::new();
    f.register(Provider::Claude);
    for (scenario, code) in [
        (json!({"hang":true}), "timeout"),
        (json!({"flood":true}), "output_limit"),
    ] {
        f.scenario(scenario);
        let start = Instant::now();
        let (_, status) = f.run(&["status", "claude-main"]);
        assert_eq!(status["diagnostic"]["code"], code, "{status}");
        assert!(start.elapsed() < STATUS_DEADLINE);
    }
    f.scenario(json!({}));
    let (_, status) = f.run(&["status", "claude-main"]);
    assert_eq!(status["state"], "missing");
}

#[test]
fn conflicting_environment_and_invoking_project_are_not_inherited() {
    let f = Fixture::new();
    f.register(Provider::Claude);
    let evidence = f.tmp.path().join("environment.json");
    f.scenario(json!({"evidence":evidence}));
    let out = f
        .command()
        .args(["status", "claude-main"])
        .env("ANTHROPIC_API_KEY", "secret-canary")
        .env("CODEX_ACCESS_TOKEN", "secret-canary")
        .env("AWS_PROFILE", "secret-canary")
        .current_dir(f.tmp.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    let value: Value = serde_json::from_slice(&fs::read(evidence).unwrap()).unwrap();
    assert!(!value.to_string().contains("secret-canary"));
    assert!(value["cwd"].as_str().unwrap().ends_with("/work"));
    assert_eq!(value["config_sources"], true);
}

#[test]
fn missing_executable_and_binding_change_are_explicit() {
    let f = Fixture::new();
    f.register(Provider::Claude);
    fs::remove_file(&f.claude).unwrap();
    let (_, status) = f.run(&["status", "claude-main"]);
    assert_eq!(status["diagnostic"]["code"], "missing_binary");
    fs::write(
        &f.config,
        "auth_profiles: {claude-main: {provider: codex, method: subscription}}\n",
    )
    .unwrap();
    let (out, error) = f.run(&["status", "claude-main"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(error["error"]["code"], "binding_conflict");
}

#[test]
fn codex_shutdown_and_best_effort_versions() {
    let f = Fixture::new();
    f.register(Provider::Codex);
    let marker = f.tmp.path().join("shutdown");
    f.scenario(json!({"shutdown_marker":marker,"version":"codex-cli 0.155.0-alpha.2"}));
    let (_, status) = f.run(&["status", "codex-main"]);
    assert_eq!(status["state"], "missing");
    assert_eq!(status["provider_version"], "0.155.0-alpha.2");
    assert_eq!(fs::read_to_string(marker).unwrap(), "graceful EOF");
    f.scenario(json!({"version":"unknown provider build"}));
    let (_, status) = f.run(&["status", "codex-main"]);
    assert_eq!(status["state"], "missing");
    assert!(status["provider_version"].is_null());
    assert!(status["diagnostic"].is_null());
    assert_eq!(status["warnings"][0]["code"], "version_unrecognized");
}

#[test]
fn slow_codex_shutdown_retains_status_and_cleans_up() {
    let f = Fixture::new();
    f.register(Provider::Codex);
    let ready = f.tmp.path().join("ready");
    for state in ["missing", "authenticated"] {
        f.scenario(json!({"state":state,"shutdown_hang":true,"ready":ready}));
        let start = Instant::now();
        let (_, status) = f.run(&["status", "codex-main"]);
        assert_eq!(status["state"], state, "{status}");
        assert!(status["diagnostic"].is_null());
        assert_eq!(status["warnings"][0]["code"], "provider_shutdown_forced");
        assert!(start.elapsed() < support::wait::SHUTDOWN_TIMEOUT);
        let pid: i32 = fs::read_to_string(&ready).unwrap().parse().unwrap();
        // SAFETY: signal 0 only checks existence; it never signals the reaped helper.
        assert_eq!(unsafe { libc::kill(-pid, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }
}

#[test]
#[ignore = "Requires installed native provider CLIs; uses empty disposable profile homes"]
fn native_signed_out_status() {
    let f = Fixture::new();
    f.register(Provider::Codex);
    f.register(Provider::Claude);
    for profile in ["codex-main", "claude-main"] {
        let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
            .arg("--config")
            .arg(&f.config)
            .arg("--state-dir")
            .arg(&f.state.root)
            .args(["auth", "status", profile])
            .output()
            .unwrap();
        let status: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(output.status.success());
        assert_eq!(status["state"], "missing", "{status}");
    }
}

#[test]
fn unsupported_state_reports_fresh_sign_in_before_any_native_or_root_writes() {
    let f = Fixture::new();
    let binding = f.register(Provider::Claude);
    let path = f.state.root.join("index.json");
    let original: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for defect in [
        IndexDefect::MissingField,
        IndexDefect::UnknownField,
        IndexDefect::UnknownState,
    ] {
        let mut old = original.clone();
        match defect {
            IndexDefect::MissingField => {
                old.as_object_mut().unwrap().remove("generations");
            }
            IndexDefect::UnknownField => old["unexpected"] = json!(true),
            IndexDefect::UnknownState => {
                old["generations"][binding.id.to_string()]["state"] = json!("unknown");
            }
        }
        fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
        let before = fs::read(&path).unwrap();
        for profile in ["claude-main", "codex-main"] {
            let (out, report) = f.run(&["recover", profile]);
            assert_eq!(out.status.code(), Some(1));
            assert_eq!(report["error"]["code"], "unsupported_schema");
            assert!(
                report["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("--state-dir")
            );
            let repair = login_pty_args(&f, &["repair", profile, "--reauthenticate"], false);
            assert_eq!(
                terminal_report(&repair)["error"]["code"],
                "unsupported_schema"
            );
        }
        let (out, report) = f.run(&[
            "prune",
            "claude-main",
            "--generation",
            &binding.id.to_string(),
        ]);
        assert_eq!(out.status.code(), Some(1));
        assert_eq!(report["error"]["code"], "unsupported_schema");
        for (binary, args) in [
            (env!("CARGO_BIN_EXE_hivectl"), vec!["daemon", "start"]),
            (env!("CARGO_BIN_EXE_hive-daemon"), vec![]),
            (
                env!("CARGO_BIN_EXE_hivectl"),
                vec!["compact", "--before", "0"],
            ),
        ] {
            let mut command = support::command(binary);
            if args.first() != Some(&"compact") {
                command.arg("--config").arg(&f.config);
            }
            let out = command
                .arg("--state-dir")
                .arg(&f.state.root)
                .args(args)
                .output()
                .unwrap();
            assert_eq!(out.status.code(), Some(1));
            let report: Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(report["error"]["code"], "unsupported_schema");
            assert!(!f.state.root.join("runtime").exists());
        }
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(!f.state.root.join("native-probes").exists());
        assert!(!f.state.root.join("native-probes.lock").exists());
    }
}

#[test]
fn replacement_preflight_leaves_missing_and_unregistered_roots_absent() {
    let f = Fixture::new();
    let destination = f.tmp.path().join("replacement");
    for profile in ["claude-main", "codex-main"] {
        let out = login_pty_args(
            &f,
            &[
                "repair",
                profile,
                "--reauthenticate",
                "--new-state-dir",
                destination.to_str().unwrap(),
            ],
            false,
        );
        assert_eq!(out.status.code(), Some(1));
        assert!(!f.state.root.exists());
        assert!(!destination.exists());
        let out = login_pty_args(&f, &["repair", profile, "--reauthenticate"], false);
        assert_eq!(terminal_report(&out)["error"]["code"], "login_required");
        assert!(!f.state.root.exists());
    }
}

#[test]
fn missing_index_does_not_adopt_an_old_runtime_database() {
    let f = Fixture::new();
    hive::state::directory(&f.state.root).unwrap();
    let runtime = f.state.root.join("runtime");
    hive::state::directory(&runtime).unwrap();
    let path = runtime.join("runtime.sqlite");
    hive::state::private_file(&path, true).unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE retained(value TEXT); INSERT INTO retained VALUES('history');")
        .unwrap();
    drop(db);
    let before = fs::read(&path).unwrap();
    for profile in ["claude-main", "codex-main"] {
        let out = login_pty_args(&f, &["login", profile], false);
        assert_eq!(terminal_report(&out)["error"]["code"], "unsupported_schema");
        assert!(!f.state.root.join("index.json").exists());
        assert!(!f.state.root.join("profiles").exists());
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}

#[test]
fn codex_replacement_isolates_unreadable_metadata_without_rewriting_it() {
    let f = Fixture::new();
    f.register(Provider::Codex);
    let index = f.state.root.join("index.json");
    let damaged = b"{\"broken\":true}";
    fs::write(&index, damaged).unwrap();
    let destination = f.tmp.path().join("replacement");
    let out = login_pty_args(
        &f,
        &[
            "repair",
            "codex-main",
            "--reauthenticate",
            "--new-state-dir",
            destination.to_str().unwrap(),
        ],
        false,
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(terminal_report(&out)["outcome"], "generation_replaced");
    assert_eq!(fs::read(&index).unwrap(), damaged);
    assert!(!f.state.root.join("native-probes").exists());
    let retired: Value =
        serde_json::from_slice(&fs::read(destination.join("retired-controller.json")).unwrap())
            .unwrap();
    assert!(retired["metadata_unavailable"].is_string());
}

#[test]
fn join_wait_timeout_is_rejected_before_state_access() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("unused-state");
    for seconds in ["0", "601", "3600"] {
        let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
            .arg("--state-dir")
            .arg(&state)
            .args(["wait", "--join", "unused", "--timeout", seconds])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["error"]["field"], "--timeout");
        assert!(String::from_utf8_lossy(&output.stderr).contains("timeout"));
        assert!(!state.exists());
    }
}

enum IndexDefect {
    MissingField,
    UnknownField,
    UnknownState,
}

#[test]
fn provider_ranges_are_reported_and_newer_claude_can_log_in() {
    let f = Fixture::new();
    f.register(Provider::Codex);
    for (version, expected) in [
        ("0.153.0", "unsupported"),
        ("0.161.0", "tested"),
        ("0.162.1", "untested"),
    ] {
        f.scenario(json!({"version":format!("codex-cli {version}")}));
        let (_, status) = f.run(&["status", "codex-main"]);
        assert_eq!(status["provider_support"]["support"], expected, "{status}");
        assert_eq!(
            status["diagnostic"]["code"] == "unsupported_provider",
            expected == "unsupported"
        );
    }
    f.scenario(json!({"version":"2.9.999 (Claude Code)"}));
    let output = f.pty_command(&["login", "claude-main"]).output().unwrap();
    let report = terminal_report(&output);
    assert!(output.status.success(), "{report}");
    assert_eq!(report["state"], "authenticated");
    assert_eq!(
        report["provider_support"]["support"], "untested",
        "{report}"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("fake-login: enter confirmation"));
}

#[test]
fn auth_pins_the_executable_when_its_alias_changes_during_version_probe() {
    let f = Fixture::new();
    let original = f.tmp.path().join("codex-original");
    let replacement = f.tmp.path().join("codex-replacement");
    let script = fs::read_to_string(&f.codex).unwrap();
    let (shebang, body) = script.split_once('\n').unwrap();
    fs::rename(&f.codex, &original).unwrap();
    fs::write(&original, format!(
        "{shebang}\nimport os, sys\nif '--version' in sys.argv:\n    os.unlink({})\n    os.symlink({}, {})\n{body}",
        json!(f.codex), json!(replacement), json!(f.codex)
    )).unwrap();
    fs::write(&replacement, "#!/bin/sh\nexit 71\n").unwrap();
    fs::set_permissions(&replacement, fs::Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::symlink(&original, &f.codex).unwrap();
    let output = f.pty_command(&["login", "codex-main"]).output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fs::read_link(&f.codex).unwrap(), replacement);
    let report = terminal_report(&output);
    assert_eq!(report["state"], "authenticated");
    assert_eq!(
        report["provider_executable"],
        json!(original.canonicalize().unwrap())
    );
}
