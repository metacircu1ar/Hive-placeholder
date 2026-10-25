use super::*;

pub(super) fn fixture() -> Harness {
    Harness::builder(Provider::Claude).build()
}
pub(super) fn spawn(f: &Harness, body: &str) -> String {
    let value = f.call(json!({"op":"spawn","role":"claude","cwd":f.tmp.path(),"body":body}));
    value["agent_id"]
        .as_str()
        .unwrap_or_else(|| panic!("{value}"))
        .into()
}
enum InvalidProof {
    Malformed,
    UnknownField,
    Unsafe,
}

#[test]
fn invalid_optional_steering_proof_does_not_block_claude_turns() {
    for (case, code) in [
        (InvalidProof::Malformed, "state_inconsistent"),
        (InvalidProof::UnknownField, "state_inconsistent"),
        (InvalidProof::Unsafe, "unsafe_state"),
    ] {
        let mut f = fixture();
        f.start();
        let proof = f.root.join("runtime/claude-steering-proof.json");
        match case {
            InvalidProof::Unsafe => {
                std::os::unix::fs::symlink(f.tmp.path().join("missing-proof"), &proof).unwrap();
            }
            InvalidProof::Malformed | InvalidProof::UnknownField => {
                let contents = match case {
                    InvalidProof::Malformed => "{invalid",
                    InvalidProof::UnknownField => r#"{"unexpected":true,"proof":null}"#,
                    InvalidProof::Unsafe => unreachable!(),
                };
                fs::write(&proof, contents).unwrap();
                fs::set_permissions(&proof, fs::Permissions::from_mode(0o600)).unwrap();
            }
        }
        let id = spawn(&f, "hold");
        let running = f.wait(&id, |a| a["state"] == "running");
        assert_ne!(running["steering_supported"], true);
        assert_eq!(running["steering_disabled_reason"], "proof_invalid");
        assert_eq!(running["steering_proof_error_code"], code);
        assert!(running["error"].is_null());
        let rejected = f.call(json!({"op":"send","target":id,"body":"guidance","mode":"steer"}));
        assert_eq!(rejected["error"]["code"], "unsupported_operation");
        let events = f.read(&id);
        assert!(
            events["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["type"] == "session_ready"
                    && e["data"]["steering_proof_error_code"] == code),
            "{events}"
        );
        f.ok(json!({"op":"stop","target":id}));
        f.wait(&id, |a| a["state"] == "stopped");
        fs::remove_file(proof).unwrap();
        f.ok(json!({"op":"send","target":id,"body":"after-proof-removal"}));
        f.ok(json!({"op":"resume","target":id}));
        answered(&f, &id, "answer:after-proof-removal");
        let resumed = f.wait(&id, |a| a["state"] == "idle");
        assert_eq!(resumed["steering_disabled_reason"], "proof_missing");
        assert!(resumed["steering_proof_error_code"].is_null());
        f.ok(json!({"op":"terminate","target":id}));
    }
}
#[test]
fn claude_persists_resume_and_runs_beside_codex() {
    let mut f = fixture();
    f.start();
    let codex = f.spawn("hello");
    let id = spawn(&f, "one");
    answered(&f, &codex, "turn_finished");
    answered(&f, &id, "answer:one");
    let initial = f.settled(&id);
    assert_eq!(initial["profile"]["provider"], "claude");
    assert_eq!(initial["native_input_attempted"], true);
    assert!(initial["steering_supported"] != true);
    // A status client need not resolve local configuration. Verification must
    // still name the daemon's actual context, with an absolute state directory.
    let status = support::command(env!("CARGO_BIN_EXE_hivectl"))
        .current_dir(f.tmp.path())
        .env("XDG_CONFIG_HOME", "unused-relative-config")
        .arg("--state-dir")
        .arg(f.root.strip_prefix(f.tmp.path()).unwrap())
        .args(["status", &id])
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stdout)
    );
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    let argv = &status["steering_verification"][0]["argv"];
    assert_eq!(argv[2], json!(f.config.canonicalize().unwrap()));
    assert_eq!(argv[4], json!(f.root));
    assert_eq!(argv[6], json!(f.tmp.path().join("claude")));
    let capabilities = f.call(json!({"op":"ping"}));
    let providers = capabilities["capabilities"]["providers"]
        .as_array()
        .unwrap();
    let claude = providers
        .iter()
        .find(|p| p["provider"] == "claude")
        .unwrap();
    assert_eq!(claude["steering_policy"], "verified_canary");
    assert_eq!(claude["read_only_shell"], false);
    assert!(f.call(json!({"op":"stop","target":id}))["error"].is_null());
    f.wait(&id, |a| a["state"] == "stopped");
    assert!(f.call(json!({"op":"send","target":id,"body":"two"}))["error"].is_null());
    f.ok(json!({"op":"resume","target":id}));
    answered(&f, &id, "answer:two");
    let resumed = f.wait(&id, |a| {
        a["state"] == "idle" && a["execution_id"] != initial["execution_id"]
    });
    assert_eq!(resumed["thread_id"], initial["thread_id"]);
    let launches: Vec<Value> = fs::read_to_string(home(&f).join("mock-launches.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(launches.len(), 2);
    assert_eq!(launches[0]["session"], initial["thread_id"]);
    assert_eq!(launches[1]["session"], initial["thread_id"]);
    assert_eq!(launches[0]["resume"], false);
    assert_eq!(launches[1]["resume"], true);
    f.ok(json!({"op":"terminate","target":id}));
    f.wait(&id, |a| a["state"] == "stopped");
    f.ok(json!({"op":"terminate","target":codex}));
}
#[test]
fn claude_interrupt_waits_for_result_and_can_continue() {
    let mut f = fixture();
    f.start();
    let id = spawn(&f, "hold");
    let running = f.wait(&id, |a| a["state"] == "running");
    let unsupported = f.call(json!({"op":"send","target":id,"body":"steer","mode":"steer"}));
    assert_eq!(unsupported["error"]["code"], "unsupported_operation");
    f.ok(json!({"op":"interrupt","target":id}));
    f.wait(&id, |a| a["state"] == "idle");
    f.ok(json!({"op":"send","target":id,"body":"continued"}));
    answered(&f, &id, "answer:continued");
    let done = f.wait(&id, |a| a["state"] == "idle");
    assert_eq!(done["thread_id"], running["thread_id"]);
    f.ok(json!({"op":"terminate","target":id}));
    f.wait(&id, |a| a["state"] == "stopped");
}

#[test]
fn claude_interrupted_exit_preserves_queued_next_message() {
    let mut f = fixture();
    f.start();
    let id = spawn(&f, "hold-error-interrupt-exit");
    let initial = f.wait(&id, |a| a["state"] == "running");
    f.ok(json!({"op":"send","target":id,"body":"after-exit"}));
    f.ok(json!({"op":"interrupt","target":id}));
    let mut observation = String::new();
    let completed = support::wait::try_wait_until(
        support::wait::TEST_TIMEOUT,
        support::wait::POLL_INTERVAL,
        || {
            let messages = f.mailbox(&id);
            let items = messages["messages"].as_array().unwrap();
            let first = items
                .iter()
                .find(|m| m["body"] == "hold-error-interrupt-exit")
                .unwrap();
            let next = items.iter().find(|m| m["body"] == "after-exit").unwrap();
            assert_ne!(next["state"], "uncertain", "{messages}");
            assert_ne!(next["state"], "failed", "{messages}");
            if first["state"] == "interrupted" && next["state"] == "completed" {
                return Some(());
            }
            let a = f.call(json!({"op":"status","target":id}))["agent"].clone();
            if a["state"] == "failed" {
                // A fenced call may report unavailability, but its durable input
                // remains accepted and an explicit exact-session resume can send it.
                assert_eq!(next["state"], "accepted", "{messages}");
                f.ok(json!({"op":"stop","target":id}));
                f.wait(&id, |a| a["state"] == "stopped");
                f.ok(json!({"op":"resume","target":id}));
            }
            observation = format!("{a}; {messages}");
            None
        },
    );
    assert!(completed.is_some(), "{observation}");
    let final_agent = f.wait(&id, |a| a["state"] == "idle");
    assert_eq!(final_agent["thread_id"], initial["thread_id"]);
    assert_ne!(final_agent["execution_id"], initial["execution_id"]);
    f.ok(json!({"op":"terminate","target":id}));
    f.wait(&id, |a| a["state"] == "stopped");
    let status = f.call(json!({"op":"ping"}));
    assert_eq!(
        status["runtime_resources"]["recovered_unresolved_activity_profiles"], 0,
        "{status}"
    );
    let events = f.read(&id);
    let shutdowns: Vec<_> = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == "provider_shutdown")
        .collect();
    assert!(
        shutdowns.len() >= 2 && shutdowns.iter().all(|e| e["data"]["confirmed"] == true),
        "{events}"
    );
}
#[test]
fn claude_coordination_uses_shared_mailbox_and_parent_cleanup() {
    let mut f = fixture();
    f.start();
    let id = spawn(&f, "spawn-reviewer");
    let done = f.wait(&id, |a| {
        a["task_outcome"] == "completed" && a["state"] == "stopped"
    });
    assert_eq!(done["result"], "reviewed");
    assert!(done["error"].is_null(), "{done}");
    let children = f.call(json!({"op":"agents","parent":id}));
    let child = &children["agents"][0]["id"];
    f.wait(child.as_str().unwrap(), |a| a["state"] == "stopped");
}

pub(super) fn home(f: &Harness) -> PathBuf {
    f.provider_home(Provider::Claude)
}
pub(super) fn answered(f: &Harness, id: &str, text: &str) {
    let mut events = Value::Null;
    let ready = support::wait::try_wait_until(
        support::wait::TEST_TIMEOUT,
        support::wait::POLL_INTERVAL,
        || {
            events = f.read(id);
            events["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|event| {
                    event["type"] == text
                        || (event["type"] == "assistant_message"
                            && event["data"]["text"]
                                .as_str()
                                .is_some_and(|value| value.contains(text)))
                })
                .then_some(())
        },
    );
    assert!(
        ready.is_some(),
        "{events}; shell: {:?}",
        fs::read_to_string(f.tmp.path().join("shell-diagnostic"))
    );
}
#[test]
fn claude_shell_children_are_gone_before_turn_completion() {
    let mut f = fixture();
    f.start();
    let id = spawn(&f, "shell-guard");
    answered(&f, &id, "shell-clean");
    let pid = fs::read_to_string(f.tmp.path().join("background.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(hive::state::ProcessIdentity::read(pid).unwrap().is_none());
    f.ok(json!({"op":"terminate","target":id}));
    let stopped = f.wait(&id, |a| a["state"] == "stopped");
    assert!(stopped["error"].is_null(), "{stopped}");
}
#[test]
fn claude_rejected_effort_keeps_profile_available() {
    let mut f = fixture();
    f.update_scenario(Provider::Claude, |value| {
        value["mismatch_effort"] = json!(true);
    });
    f.start();
    let id = spawn(&f, "one");
    let failed = f.wait(&id, |a| a["state"] == "failed");
    assert_eq!(failed["error"], "effort_mismatch");
    assert_ne!(failed["native_input_attempted"], true);
    f.update_scenario(Provider::Claude, |value| {
        value.as_object_mut().unwrap().remove("mismatch_effort");
    });
    f.ok(json!({"op":"stop","target":id}));
    f.wait(&id, |a| a["state"] == "stopped");
    f.ok(json!({"op":"resume","target":id}));
    answered(&f, &id, "answer:one");
    let resumed = f.wait(&id, |a| a["state"] == "idle");
    assert_eq!(resumed["thread_id"], failed["thread_id"]);
    assert_eq!(resumed["native_input_attempted"], true);
    let launches: Vec<Value> = fs::read_to_string(home(&f).join("mock-launches.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(launches.len(), 2);
    assert_eq!(launches[1]["resume"], false);
    f.ok(json!({"op":"terminate","target":id}));

    let next = spawn(&f, "two");
    answered(&f, &next, "answer:two");
    f.ok(json!({"op":"terminate","target":next}));
}
#[test]
fn claude_missing_resume_never_creates_a_fresh_session_or_quarantines_profile() {
    let mut f = fixture();
    f.start();
    let id = spawn(&f, "one");
    answered(&f, &id, "answer:one");
    f.ok(json!({"op":"stop","target":id}));
    let stopped = f.wait(&id, |a| a["state"] == "stopped");
    fs::remove_file(home(&f).join(format!(
        "mock-session-{}",
        stopped["thread_id"].as_str().unwrap()
    )))
    .unwrap();
    f.ok(json!({"op":"resume","target":id}));
    let failed = f.wait(&id, |a| a["state"] == "failed");
    assert_eq!(failed["thread_id"], stopped["thread_id"]);
    assert_eq!(failed["native_input_attempted"], true);
    assert_ne!(failed["error"], "recovery_required", "{failed}");
    // A rejected resume exits before launch logging; a fresh-session fallback
    // would create a second launch entry and violate this assertion.
    assert_eq!(
        fs::read_to_string(home(&f).join("mock-launches.jsonl"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    let next = spawn(&f, "two");
    answered(&f, &next, "answer:two");
    f.ok(json!({"op":"terminate","target":next}));
}

#[test]
fn claude_stray_socket_does_not_poison_registered_command() {
    let mut f = fixture();
    f.start();
    let id = spawn(&f, "shell-stray");
    answered(&f, &id, "shell-clean");
    f.ok(json!({"op":"terminate","target":id}));
    let stopped = f.wait(&id, |a| a["state"] == "stopped");
    assert!(stopped["error"].is_null(), "{stopped}");
}
#[test]
fn claude_forced_helper_loss_uses_kernel_cleanup_evidence() {
    let mut f = fixture();
    f.start();
    let id = spawn(&f, "shell-killed");
    answered(&f, &id, "shell-clean");
    let leader = fs::read_to_string(f.tmp.path().join("leader.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(
        hive::state::ProcessIdentity::read(leader)
            .unwrap()
            .is_none()
    );
    f.ok(json!({"op":"terminate","target":id}));
    let stopped = f.wait(&id, |a| a["state"] == "stopped");
    assert!(stopped["error"].is_null(), "{stopped}");
}

#[test]
fn claude_backpressure_does_not_interleave_native_reply_with_prompt() {
    let mut f = fixture();
    f.update_scenario(Provider::Claude, |value| {
        value["backpressure"] = json!(true);
    });
    f.start();
    let id = spawn(&f, &format!("backpressure:{}", "x".repeat(65_523)));
    answered(&f, &id, "large-input-intact");
    support::wait::wait_until(
        Duration::from_secs(5),
        Duration::from_millis(10),
        "provider gate",
        || {
            home(&f)
                .join("backpressure-confirmed")
                .exists()
                .then_some(())
        },
    );
    f.ok(json!({"op":"terminate","target":id}));
    let stopped = f.wait(&id, |a| a["state"] == "stopped");
    assert!(stopped["error"].is_null(), "{stopped}");
}
#[test]
fn claude_interrupt_result_waits_for_attached_shell_cleanup() {
    let mut f = fixture();
    f.start();
    let id = spawn(&f, "shell-interrupt");
    answered(&f, &id, "shell-running");
    f.ok(json!({"op":"interrupt","target":id}));
    f.settled(&id);
    let leader = fs::read_to_string(f.tmp.path().join("leader.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(
        hive::state::ProcessIdentity::read(leader)
            .unwrap()
            .is_none()
    );
    f.ok(json!({"op":"terminate","target":id}));
    let stopped = f.wait(&id, |a| a["state"] == "stopped");
    assert!(stopped["error"].is_null(), "{stopped}");
}
#[test]
fn claude_lost_helper_with_live_group_cannot_confirm_completion() {
    let mut f = fixture();
    f.start();
    let id = spawn(&f, "shell-retained");
    // The fixture allows 15s for helper startup, then deliberately retains the
    // group for 4s before reaping it. The generic 15s observation limit cannot
    // cover that permitted path plus fault detection and provider shutdown.
    let failed = f.wait_timeout(&id, Duration::from_secs(45), |a| a["state"] == "failed");
    assert!(!failed["error"].is_null(), "{failed}");
    let events = f.read(&id);
    assert!(
        !events["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["type"] == "turn_finished"),
        "{events}"
    );
    assert!(
        events["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |event| event["type"] == "provider_shutdown" && event["data"]["confirmed"] == false
            ),
        "{events}"
    );
    support::wait::wait_until(
        Duration::from_secs(10),
        Duration::from_millis(10),
        "provider gate",
        || f.tmp.path().join("retained-cleaned").exists().then_some(()),
    );
}

#[test]
#[ignore = "Requires installed native Claude; uses an empty isolated profile, no inference"]
fn native_claude_signed_out_initialization_does_not_quarantine_profile() {
    let mut f = fixture();
    let binary = std::env::var_os("HIVE_TEST_CLAUDE_BIN")
        .expect("Set an absolute native Claude executable path");
    assert!(std::path::Path::new(&binary).is_absolute());
    fs::remove_file(f.tmp.path().join("claude")).unwrap();
    std::os::unix::fs::symlink(binary, f.tmp.path().join("claude")).unwrap();
    f.start();
    for _ in 0..2 {
        let id = spawn(
            &f,
            "This prompt must never be submitted: the profile is signed out.",
        );
        let failed = f.wait(&id, |a| a["state"] == "failed");
        assert_eq!(failed["error"], "authentication_required", "{failed}");
        assert_ne!(failed["native_input_attempted"], true);
        let events = f.read(&id);
        assert!(
            !events["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|event| event["type"] == "turn_started"),
            "{events}"
        );
        let daemon = f.call(json!({"op":"ping"}));
        assert_eq!(
            daemon["runtime_resources"]["recovered_unresolved_activity_profiles"], 0,
            "{daemon}"
        );
    }
}

#[test]
fn claude_native_api_error_and_error_interrupt_keep_profile_reusable() {
    let mut f = fixture();
    f.start();
    for body in ["api-error", "api-error-exit"] {
        let id = spawn(&f, body);
        f.wait(&id, |a| a["state"] == "failed");
        answered(&f, &id, "provider_shutdown");
        let events = f.read(&id);
        assert!(
            events["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(
                    |event| event["type"] == "turn_finished" && event["data"]["status"] == "failed"
                ),
            "{events}"
        );
        let messages = f.mailbox(&id);
        assert_eq!(messages["messages"][0]["state"], "failed", "{messages}");
        assert!(
            events["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|event| event["type"] == "provider_shutdown"
                    && event["data"]["confirmed"] == true
                    && event["data"]["exit_code"] == 1),
            "{events}"
        );
    }
    let next = spawn(&f, "hold-error-interrupt");
    f.wait(&next, |a| a["state"] == "running");
    f.ok(json!({"op":"interrupt","target":next}));
    // Terminal errors retire this physical process before native can exit under
    // a new input. An unloaded conversation is eligible for the next message.
    f.wait(&next, |a| a["state"] == "idle" || a["state"] == "stopped");
    f.ok(json!({"op":"send","target":next,"body":"continued"}));
    answered(&f, &next, "answer:continued");
    f.ok(json!({"op":"terminate","target":next}));
    let stopped = f.wait(&next, |a| a["state"] == "stopped");
    assert!(stopped["error"].is_null(), "{stopped}");
}

#[test]
fn claude_shell_resets_inherited_ignored_sigchld() {
    let mut f = fixture();
    f.start();
    let id = spawn(&f, "shell-sigchld");
    answered(&f, &id, "shell-clean");
    f.ok(json!({"op":"terminate","target":id}));
    let stopped = f.wait(&id, |a| a["state"] == "stopped");
    assert!(stopped["error"].is_null(), "{stopped}");
}

#[test]
fn claude_helper_cancellation_never_opens_gate_and_registration_kill_spawns_nothing() {
    let f = fixture();
    let output = support::command(f.tmp.path().join("python3"))
        .arg("-c")
        .arg(include_str!("../fixtures/claude_shell_gate.py"))
        .arg(env!("CARGO_BIN_EXE_hivectl"))
        .current_dir(f.tmp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn skill_launch(f: &Harness) -> Value {
    serde_json::from_slice(
        &fs::read(f.provider_home(Provider::Claude).join("skill-launch.json")).unwrap(),
    )
    .unwrap()
}

#[test]
fn claude_skills_option_runs_a_forked_skill_under_the_tool_policy() {
    let mut f = fixture();
    f.start();
    let value = f.call(json!({"op":"spawn","role":"claude","cwd":f.tmp.path(),
        "body":"skill:fork","options":{"claude.skills":true}}));
    let id = value["agent_id"]
        .as_str()
        .unwrap_or_else(|| panic!("{value}"))
        .to_owned();
    let agent = f.settled(&id);
    assert!(agent["error"].is_null(), "{agent}");
    let launch = skill_launch(&f);
    assert_eq!(launch["disable_slash_commands"], false, "{launch}");
    assert!(
        launch["tools"].as_str().unwrap().ends_with(",Skill"),
        "{launch}"
    );
    // The fork's Read and the Skill call settle in history, then the reply.
    let events = f.read(&id);
    let kinds: Vec<(&str, &str)> = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| {
            let id = e["data"]["item"]["id"]
                .as_str()
                .or(e["data"]["text"].as_str())?;
            Some((e["type"].as_str().unwrap(), id))
        })
        .collect();
    assert_eq!(
        kinds,
        [
            ("tool_completed", "skill-read"),
            ("tool_completed", "skill-tool"),
            ("assistant_message", "skill-forked")
        ],
        "{events}"
    );
    // Without the option the same call is denied and the session stays usable.
    let id = spawn(&f, "skill:fork");
    let agent = f.settled(&id);
    assert!(agent["error"].is_null(), "{agent}");
    // The launch file is shared; read it only once this agent's turn ran.
    support::wait::wait_until(
        support::wait::TEST_TIMEOUT,
        support::wait::POLL_INTERVAL,
        "the denied skill call",
        || {
            f.read(&id)
                .to_string()
                .contains("skill-denied")
                .then_some(())
        },
    );
    let launch = skill_launch(&f);
    assert_eq!(launch["disable_slash_commands"], true, "{launch}");
    assert!(
        !launch["tools"].as_str().unwrap().contains("Skill"),
        "{launch}"
    );
    assert!(f.read(&id).to_string().contains("skill-denied"));
    f.shutdown();
}

#[test]
fn read_only_claude_runs_only_checked_git_commands() {
    let mut f = fixture();
    f.start();
    let repo = f.tmp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    let git = |args: &[&str]| {
        let output = support::command("git")
            .args(args)
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}");
    };
    git(&["init", "-q"]);
    fs::write(repo.join("tracked.txt"), "first").unwrap();
    git(&["add", "tracked.txt"]);
    // Same content, newer time: an ordinary status would refresh the index.
    fs::OpenOptions::new()
        .write(true)
        .open(repo.join("tracked.txt"))
        .unwrap()
        .set_modified(std::time::SystemTime::now() + Duration::from_secs(60))
        .unwrap();
    let index = fs::read(repo.join(".git/index")).unwrap();
    let agent = |body: &str| {
        let value = f.call(json!({"op":"spawn","role":"claude","cwd":repo,
            "sandbox":"read-only","body":body}));
        value["agent_id"]
            .as_str()
            .unwrap_or_else(|| panic!("{value}"))
            .to_owned()
    };
    let text = |id: &str| f.read(id).to_string();
    // The checked command runs through the shell guard and reports the change.
    let reader = agent("git-status");
    answered(&f, &reader, "git exit 0");
    assert!(
        text(&reader).contains("A  tracked.txt"),
        "{}",
        text(&reader)
    );
    // Shell syntax is refused before anything runs.
    let writer = agent("git-denied");
    answered(&f, &writer, "git denied");
    assert!(!repo.join("pwned").exists());
    // GIT_OPTIONAL_LOCKS=0 keeps even status from refreshing the index.
    assert_eq!(fs::read(repo.join(".git/index")).unwrap(), index);
    f.shutdown();
}
