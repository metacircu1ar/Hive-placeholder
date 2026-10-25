//! Credential lifecycle through a real CLI child and deterministic native providers.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "../../tests/cli/resources.rs"]
mod resources;
use crate::integration_support as support;
use crate::{
    config::{Method, Profile, Provider},
    state::State,
};
use resources::{
    LoginRelease, PROBE_EXIT_TIMEOUT, PROBE_POLL_INTERVAL, PROBE_READY_TIMEOUT, ProbeRelease,
    RECOVERY_TIMEOUT,
};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{Duration, Instant},
};
use support::wait::wait_until;
const HANGUP_DEADLINE: Duration = Duration::from_secs(10);
include!("../../tests/cli/fixture.rs");
fn cli_binary() -> &'static Path {
    child::binary()
}
pub(crate) mod child;

#[test]
fn failed_status_probe_does_not_poison_the_credential_namespace() {
    for close in [false, true] {
        let f = Fixture::new();
        f.register(Provider::Claude);
        let ready = f.tmp.path().join("ready");
        let stop = f.tmp.path().join("stop");
        f.scenario(json!({"close_fds":close,"hang":true,"ready":ready,"stop_file":stop}));
        // Keep the CLI's report: if the probe never gets ready, it tells a probe
        // timeout in the CLI apart from a slow probe start.
        let report = f.tmp.path().join("status-report.json");
        let mut parent = support::ChildGuard::new(
            f.command()
                .args(["status", "claude-main"])
                .stdout(fs::File::create(&report).unwrap())
                .spawn()
                .unwrap(),
        );
        let mut release = ProbeRelease::new(&stop, &ready);
        if support::wait::try_wait_until(PROBE_READY_TIMEOUT, PROBE_POLL_INTERVAL, || {
            ready.exists().then_some(())
        })
        .is_none()
        {
            panic!(
                "probe not ready; CLI exited: {:?}; report: {}",
                parent.try_wait().unwrap(),
                fs::read_to_string(&report).unwrap_or_default()
            );
        }
        parent.kill().unwrap();
        parent.wait().unwrap();
        let identity = release.identity();
        fs::write(stop, "stop").unwrap();
        wait_until(
            PROBE_EXIT_TIMEOUT,
            PROBE_POLL_INTERVAL,
            "probe exit",
            || identity.absent().unwrap().then_some(()),
        );
        f.scenario(json!({}));
        let (_, status) = f.run(&["status", "claude-main"]);
        assert_eq!(status["state"], "missing");
        let (_, recovery) = f.run(&["recover", "claude-main"]);
        assert_eq!(recovery["recovery"]["runtime_usable"], true);
        // Read-only probe retains its exact ownership evidence; it is never
        // reclassified as an unfinished credential writer.
        assert!(
            recovery["recovery"]["executions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["class"] == "probe")
        );
        assert!(login_pty(&f, false).status.success());
    }
}

#[test]
fn native_login_preserves_foreground_tty_and_verifies_result() {
    let f = Fixture::new();
    for provider in ["codex-main", "claude-main"] {
        let output = f.pty_command(&["login", provider]).output().unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("authenticated"));
        let (_, status) = f.run(&["status", provider]);
        assert_eq!(status["state"], "authenticated");
    }
}

#[test]
fn raw_native_login_receives_carriage_return_unchanged() {
    let f = Fixture::new();
    f.scenario(json!({"login_raw_enter":true}));
    let out = login_pty(&f, false);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn login_refuses_conflict_and_never_accepts_exit_code_alone() {
    let f = Fixture::new();
    f.scenario(json!({"state":"api"}));
    let out = login_pty(&f, false);
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("login_refused"));
    assert!(!text.contains("enter confirmation"));
    f.scenario(json!({"login_no_credentials":true}));
    let out = login_pty(&f, false);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("login_failed"));
}

#[test]
fn cancellation_and_forced_termination_preserve_profile() {
    let f = Fixture::new();
    let out = login_pty(&f, true);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("cancelled"));
    let (_, status) = f.run(&["status", "claude-main"]);
    assert_eq!(status["state"], "missing");
    f.scenario(json!({"login_ignore_signals":true}));
    let out = login_pty(&f, true);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("recovery_required"));
    assert_eq!(terminal_report(&out)["outcome"], "verification_pending");
    let (_, status) = f.run(&["status", "claude-main"]);
    assert_eq!(status["state"], "recovery_required");
    f.scenario(json!({}));
    let (exit, recovered) = f.run(&["recover", "claude-main"]);
    assert!(exit.status.success(), "{recovered}");
    assert_eq!(recovered["recovery"]["runtime_usable"], true);
    assert!(login_pty(&f, false).status.success());
}

#[test]
fn terminal_hangup_preserves_graceful_cleanup_and_post_login_verification() {
    for (phase, block_hup) in [
        ("ready", false),
        ("complete", false),
        ("ready", true),
        ("complete", true),
    ] {
        let f = Fixture::new();
        f.scenario(json!({"login_notify_saved":true}));
        let began = Instant::now();
        let output = f
            .pty_command(&["login", "claude-main"])
            .env("HIVE_PTY_HANGUP", phase)
            .env("HIVE_PTY_BLOCK_HUP", if block_hup { "1" } else { "" })
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "hangup phase={phase}, blocked SIGHUP={block_hup}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(began.elapsed() < HANGUP_DEADLINE);
        let (_, status) = f.run(&["status", "claude-main"]);
        assert_eq!(
            status["state"],
            if phase == "ready" {
                "missing"
            } else {
                "authenticated"
            },
            "{status}"
        );
    }
}

#[test]
fn graceful_sigterm_and_double_interrupt_need_no_recovery() {
    for double in [false, true] {
        let f = Fixture::new();
        if double {
            f.scenario(json!({"login_slow_sigint":true}));
        } else {
            f.scenario(json!({"login_ignore_sigint":true}));
        }
        let out = f
            .pty_command(&["login", "claude-main"])
            .env("HIVE_PTY_CANCEL", if double { "double" } else { "1" })
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1));
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("cancelled"),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        let (_, status) = f.run(&["status", "claude-main"]);
        assert_eq!(status["state"], "missing");
    }
}

#[test]
fn login_timeout_with_graceful_exit_needs_no_recovery() {
    let f = Fixture::new();
    f.scenario(json!({"login_ignore_sigint":true}));
    let out = f
        .pty_command(&["login", "claude-main", "--login-timeout", "1"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("timeout"));
    let (_, status) = f.run(&["status", "claude-main"]);
    assert_eq!(status["state"], "missing");
}

#[test]
fn explicit_reauthentication_preserves_existing_login_on_failure() {
    let f = Fixture::new();
    assert!(login_pty(&f, false).status.success());
    let repeat = login_pty(&f, false);
    assert!(String::from_utf8_lossy(&repeat.stdout).contains("already_authenticated"));
    f.scenario(json!({"login_failure":true}));
    let out = f
        .pty_command(&["login", "claude-main", "--reauthenticate"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("enter confirmation"));
    let (_, status) = f.run(&["status", "claude-main"]);
    assert_eq!(status["state"], "authenticated");
}

#[test]
fn failed_verification_is_retryable_without_repair_and_preserves_account_binding() {
    let f = Fixture::new();
    assert!(login_pty(&f, false).status.success());
    let original = f.state.generations("claude-main").unwrap().remove(0);
    f.scenario(json!({"post_login_state":"malformed"}));
    // The initial probe also fails while credentials exist: remove only the
    // fixture marker to simulate signed-out native credentials before login.
    fs::remove_file(
        original
            .binding
            .directory
            .join("provider/fake-login-complete"),
    )
    .unwrap();
    let out = login_pty(&f, false);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        f.state.generations("claude-main").unwrap()[0]
            .credential_mutation
            .is_some()
    );
    let (_, pending) = f.run(&["recover", "claude-main"]);
    assert_eq!(pending["recovery"]["runtime_usable"], false);
    f.scenario(json!({}));
    let (out, recovered) = f.run(&["recover", "claude-main"]);
    assert!(out.status.success());
    assert_eq!(recovered["recovery"]["runtime_usable"], true, "{recovered}");
    let current = f.state.generations("claude-main").unwrap().remove(0);
    assert_eq!(current.binding.id, original.binding.id);
    assert_eq!(current.account_identity, original.account_identity);
    assert!(current.credential_mutation.is_none());
}

#[test]
fn changed_account_after_login_requires_repair_instead_of_recover_retry() {
    let f = Fixture::new();
    assert!(login_pty(&f, false).status.success());
    let original = f.state.generations("claude-main").unwrap().remove(0);
    fs::remove_file(
        original
            .binding
            .directory
            .join("provider/fake-login-complete"),
    )
    .unwrap();
    f.scenario(json!({"email":"different@example.test"}));
    let out = login_pty(&f, false);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(terminal_report(&out)["outcome"], "repair_required");
    let current = f.state.generations("claude-main").unwrap().remove(0);
    assert_eq!(current.account_identity, original.account_identity);
    assert!(current.credential_mutation.is_some());
    assert!(f.state.runtime_lease(&current.binding).is_err());
}

#[test]
fn signed_out_and_expired_post_login_results_do_not_lock_out_old_generations() {
    for state in ["missing", "reauthentication_required"] {
        let f = Fixture::new();
        assert!(login_pty(&f, false).status.success());
        let original = f.state.generations("claude-main").unwrap().remove(0);
        fs::remove_file(
            original
                .binding
                .directory
                .join("provider/fake-login-complete"),
        )
        .unwrap();
        f.scenario(json!({"post_login_state":state}));
        assert_eq!(login_pty(&f, false).status.code(), Some(1));
        let saved = f.state.generations("claude-main").unwrap().remove(0);
        assert!(
            saved.credential_mutation.is_none() && !saved.authenticated && saved.ever_authenticated
        );
        assert_eq!(saved.account_identity, original.account_identity);
        let (_, status) = f.run(&["status", "claude-main"]);
        assert_eq!(status["state"], state);
        f.scenario(json!({}));
        assert!(login_pty(&f, false).status.success());
    }
}

#[test]
fn below_minimum_claude_repair_is_refused_before_creating_a_replacement() {
    let f = Fixture::new();
    f.register(Provider::Claude);
    f.scenario(json!({"version":"2.1.284 (Claude Code)"}));
    let generations = f.state.generations("claude-main").unwrap().len();
    let output = login_pty_args(&f, &["repair", "claude-main", "--reauthenticate"], false);
    assert_eq!(output.status.code(), Some(1));
    let report = terminal_report(&output);
    assert_eq!(report["diagnostic"]["code"], "unsupported_provider");
    assert_eq!(
        f.state.generations("claude-main").unwrap().len(),
        generations
    );
    assert!(
        report["diagnostic"]["message"]
            .as_str()
            .unwrap()
            .contains("supported minimum 2.1.285")
    );
    let destination = f.tmp.path().join("fresh-controller");
    let replacement = login_pty_args(
        &f,
        &[
            "repair",
            "claude-main",
            "--reauthenticate",
            "--new-state-dir",
            destination.to_str().unwrap(),
        ],
        false,
    );
    assert_eq!(replacement.status.code(), Some(1));
    assert_eq!(
        terminal_report(&replacement)["diagnostic"]["code"],
        "unsupported_provider"
    );
    assert!(!destination.exists());
    assert!(terminal_report(&replacement)["metadata_probe_directory"].is_null());
    assert!(!fs::read_dir(f.tmp.path()).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".hive-probe-")
    }));
    assert_eq!(
        fs::read_dir(f.state.root.join("native-probes"))
            .unwrap()
            .count(),
        0
    );
    f.scenario(json!({"version_fail":true}));
    let failed = login_pty_args(&f, &["repair", "claude-main", "--reauthenticate"], false);
    assert_eq!(
        terminal_report(&failed)["diagnostic"]["code"],
        "provider_failed"
    );
    assert_eq!(
        f.state.generations("claude-main").unwrap().len(),
        generations
    );
}

#[test]
fn claude_pending_verification_accepts_newer_versions_without_a_namespace_audit() {
    let f = Fixture::new();
    f.scenario(json!({"post_login_state":"malformed"}));
    assert_eq!(login_pty(&f, false).status.code(), Some(1));
    assert!(
        f.state.generations("claude-main").unwrap()[0]
            .credential_mutation
            .is_some()
    );
    f.scenario(json!({"version":"2.9.999 (Claude Code)","state":"missing"}));
    let (out, report) = f.run(&["recover", "claude-main"]);
    assert!(out.status.success(), "{report}");
    assert!(
        f.state.generations("claude-main").unwrap()[0]
            .credential_mutation
            .is_none()
    );
    assert_eq!(
        report["credential_verification"]["provider_support"]["support"],
        "untested"
    );
    f.scenario(json!({"version":"2.9.999 (Claude Code)"}));
    assert!(login_pty(&f, false).status.success());
}

#[test]
fn newer_claude_login_and_isolated_repair_do_not_require_a_namespace_audit() {
    let f = Fixture::new();
    f.scenario(json!({"version":"2.9.999 (Claude Code)"}));
    let login = login_pty(&f, false);
    assert!(
        login.status.success(),
        "{}",
        String::from_utf8_lossy(&login.stdout)
    );
    let original = f.state.generations("claude-main").unwrap()[0].binding.id;
    let repaired = login_pty_args(&f, &["repair", "claude-main", "--reauthenticate"], false);
    let report = terminal_report(&repaired);
    assert!(repaired.status.success(), "{report}");
    assert_eq!(report["provider_support"]["support"], "untested");
    assert!(
        f.state
            .generations("claude-main")
            .unwrap()
            .iter()
            .any(|g| g.binding.id != original && g.authenticated)
    );
}

#[test]
fn in_place_verification_requires_a_fresh_exclusive_lock_description() {
    let f = Fixture::new();
    let stop = f.tmp.path().join("detached.stop");
    f.scenario(json!({"login_detached_stop":stop}));
    let _release = LoginRelease::new(&stop, &f);
    let out = login_pty(&f, false);
    assert_eq!(out.status.code(), Some(1));
    let report = terminal_report(&out);
    assert_eq!(report["outcome"], "verification_pending");
    assert_eq!(report["diagnostic"]["code"], "recovery_required");
    assert!(
        f.state.generations("claude-main").unwrap()[0]
            .credential_mutation
            .is_some()
    );
    fs::write(stop, "stop").unwrap();
    f.scenario(json!({}));
    let mut last_report = Value::Null;
    let verified =
        support::wait::try_wait_until(RECOVERY_TIMEOUT, support::wait::POLL_INTERVAL, || {
            let (out, report) = f.run(&["recover", "claude-main"]);
            if out.status.success() {
                assert_eq!(report["recovery"]["runtime_usable"], true);
                Some(())
            } else {
                last_report = report;
                None
            }
        });
    assert!(verified.is_some(), "{last_report}");
}

#[test]
fn slow_codex_shutdown_allows_login_and_verification() {
    let f = Fixture::new();
    f.scenario(json!({"shutdown_hang":true}));
    let output = login_pty_args(&f, &["login", "codex-main"], false);
    let report = terminal_report(&output);
    assert!(output.status.success(), "{report}");
    assert_eq!(report["outcome"], "authenticated");
    assert_eq!(report["warnings"][0]["code"], "provider_shutdown_forced");
}

#[test]
fn version_warning_does_not_hide_login_failure() {
    let f = Fixture::new();
    f.scenario(json!({"version":"unrecognized build","login_failure":true}));
    let output = login_pty_args(&f, &["login", "codex-main"], false);
    let report = terminal_report(&output);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(report["diagnostic"]["code"], "login_failed");
    assert_eq!(report["warnings"][0]["code"], "version_unrecognized");
    let claude = login_pty(&f, false);
    let report = terminal_report(&claude);
    assert_eq!(claude.status.code(), Some(1));
    assert_eq!(report["diagnostic"]["code"], "login_failed");
    assert_eq!(report["warnings"][0]["code"], "version_unrecognized");
}

#[test]
fn browser_lock_errors_keep_their_cause() {
    let f = Fixture::new();
    f.register(Provider::Codex);
    let path = f.state.root.join("codex-browser.lock");
    let lock = hive::state::Lock::acquire(&path).unwrap();
    let output = login_pty_args(&f, &["login", "codex-main", "--browser"], false);
    assert_eq!(
        terminal_report(&output)["error"]["code"],
        "codex_browser_login_busy"
    );
    drop(lock);
    {
        let _permissions = support::PermissionsGuard::set(&path, 0o644);
        let output = login_pty_args(&f, &["login", "codex-main", "--browser"], false);
        assert_eq!(terminal_report(&output)["error"]["code"], "unsafe_state");
    }
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(f.tmp.path().join("absent"), &path).unwrap();
    let output = login_pty_args(&f, &["login", "codex-main", "--browser"], false);
    assert_eq!(terminal_report(&output)["error"]["code"], "state_io");
}

#[test]
fn isolated_repair_after_cancelled_writer_requires_no_reboot_or_token_copy() {
    let f = Fixture::new();
    f.scenario(json!({"login_ignore_signals":true}));
    let cancelled = login_pty(&f, true);
    assert!(String::from_utf8_lossy(&cancelled.stdout).contains("recovery_required"));
    let binding = f
        .state
        .lookup(
            "claude-main",
            &Profile {
                provider: Provider::Claude,
                method: Method::Subscription,
                api_key_env: None,
                api_key_file: None,
            },
        )
        .unwrap()
        .unwrap();
    let (_, old) = f.run(&["status", "claude-main"]);
    assert_eq!(old["state"], "recovery_required");
    f.scenario(json!({}));
    let output = login_pty_args(&f, &["repair", "claude-main", "--reauthenticate"], false);
    let report = terminal_report(&output);
    assert!(output.status.success(), "{report}");
    assert_eq!(report["outcome"], "generation_replaced");
    assert_ne!(report["generation"], binding.id.to_string());
    assert!(
        !binding
            .directory
            .join("provider/fake-login-complete")
            .exists()
    );
    let (_, status) = f.run(&["status", "claude-main"]);
    assert_eq!(status["state"], "authenticated");
    assert!(f.state.runtime_lease(&binding).is_err());
}

#[test]
fn claude_replacement_preflight_never_writes_the_old_root() {
    for obstacle in [RootObstacle::ReadOnly, RootObstacle::UnsafeProbe] {
        let f = Fixture::new();
        f.register(Provider::Claude);
        let before = fs::read(f.state.root.join("index.json")).unwrap();
        let permissions = match obstacle {
            RootObstacle::ReadOnly => vec![
                support::PermissionsGuard::set(&f.state.root, 0o500),
                support::PermissionsGuard::set(f.state.root.join("index.json"), 0o400),
            ],
            RootObstacle::UnsafeProbe => {
                std::os::unix::fs::symlink(f.tmp.path(), f.state.root.join("native-probes"))
                    .unwrap();
                vec![]
            }
        };
        let destination = f.tmp.path().join("replacement");
        let out = login_pty_args(
            &f,
            &[
                "repair",
                "claude-main",
                "--reauthenticate",
                "--new-state-dir",
                destination.to_str().unwrap(),
            ],
            false,
        );
        drop(permissions);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert_eq!(terminal_report(&out)["outcome"], "generation_replaced");
        assert_eq!(fs::read(f.state.root.join("index.json")).unwrap(), before);
        assert!(!f.state.root.join("native-probes.lock").exists());
        assert!(!fs::read_dir(f.tmp.path()).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".hive-probe-")
        }));
    }
}

#[test]
fn retired_repair_preserves_pending_verification_and_actionable_guidance() {
    let f = Fixture::new();
    f.register(Provider::Claude);
    f.scenario(json!({"post_login_state":"malformed"}));
    let out = login_pty_args(&f, &["repair", "claude-main", "--reauthenticate"], false);
    assert_eq!(out.status.code(), Some(1));
    let report = terminal_report(&out);
    assert_eq!(report["outcome"], "repair_retired");
    assert_eq!(report["verification"], "pending");
    assert_eq!(report["state_directory"], json!(f.state.root));
    assert!(
        report["diagnostic"]["message"]
            .as_str()
            .unwrap()
            .contains("omit --new-state-dir")
    );
    let generation = f
        .state
        .generations("claude-main")
        .unwrap()
        .into_iter()
        .find(|g| json!(g.binding.id) == report["generation"])
        .unwrap();
    assert_eq!(generation.state, hive::state::GenerationState::Retired);
    assert!(generation.credential_mutation.is_some());
}

#[test]
fn verification_io_errors_keep_the_login_or_repair_report() {
    for repair in [false, true] {
        let f = Fixture::new();
        if repair {
            f.register(Provider::Claude);
        }
        f.scenario(json!({"post_login_corrupt_index":f.state.root.join("index.json")}));
        let args = if repair {
            vec!["repair", "claude-main", "--reauthenticate"]
        } else {
            vec!["login", "claude-main"]
        };
        let out = login_pty_args(&f, &args, false);
        assert_eq!(
            out.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        let report = terminal_report(&out);
        assert_eq!(report["verification"], "pending");
        assert_eq!(report["state_directory"], json!(f.state.root));
        assert!(report["generation"].is_string());
        assert_eq!(
            report["outcome"],
            if repair {
                "repair_finalization_failed"
            } else {
                "verification_pending"
            }
        );
        assert!(report["diagnostic"].is_object());
    }
}

#[test]
fn replacement_refuses_bad_metadata_before_probe_and_preserves_later_probe_evidence() {
    for before_probe in [true, false] {
        let f = Fixture::new();
        f.register(Provider::Claude);
        let destination = f.tmp.path().join("replacement");
        let index = f.state.root.join("index.json");
        if before_probe {
            fs::write(&index, r#"{"broken":true}"#).unwrap();
            f.scenario(json!({"version_fail":true}));
        } else {
            f.scenario(json!({"version_corrupt_index":index,"version_stray":true}));
        }
        let out = login_pty_args(
            &f,
            &[
                "repair",
                "claude-main",
                "--reauthenticate",
                "--new-state-dir",
                destination.to_str().unwrap(),
            ],
            false,
        );
        let report = terminal_report(&out);
        assert!(!destination.exists());
        assert!(!f.state.root.join("native-probes").exists());
        let wrappers: Vec<_> = fs::read_dir(f.tmp.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".hive-probe-")
            })
            .collect();
        if before_probe {
            assert_eq!(report["error"]["code"], "recovery_required");
            assert!(wrappers.is_empty());
        } else {
            assert_eq!(out.status.code(), Some(1));
            assert_eq!(report["outcome"], "repair_refused");
            assert_eq!(wrappers.len(), 1);
            assert_eq!(
                report["metadata_probe_directory"],
                json!(wrappers[0].canonicalize().unwrap())
            );
            assert!(wrappers[0].join("native-probes").exists());
        }
    }
}

enum RootObstacle {
    ReadOnly,
    UnsafeProbe,
}

fn login_pty(f: &Fixture, cancel: bool) -> Output {
    login_pty_args(f, &["login", "claude-main"], cancel)
}
