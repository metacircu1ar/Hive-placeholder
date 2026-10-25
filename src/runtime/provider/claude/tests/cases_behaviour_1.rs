//! Behaviour regressions.
use super::*;

#[test]
fn full_access_claude_allows_arbitrary_foreground_commands_and_external_edits() {
    for (tool, input) in [
        (
            "Bash",
            json!({"command":"cd /tmp && git init && git add . && git commit -m example; curl https://example.com", "dangerouslyDisableSandbox":true}),
        ),
        (
            "Write",
            json!({"file_path":"/somewhere/outside/workspace/file.txt","content":"hello"}),
        ),
        (
            "Edit",
            json!({"file_path":"/somewhere/outside/workspace/.git/config","old_string":"a","new_string":"b"}),
        ),
        (
            "NotebookEdit",
            json!({"notebook_path":"/somewhere/outside/workspace/book.ipynb"}),
        ),
    ] {
        let mut fixture = Fixture::with_extensions(None, Uuid::new_v4().to_string(), |config| {
            config.sandbox = Sandbox::FullAccess;
        });
        fixture.turn();
        let output = fixture.hook(tool, input);
        assert_eq!(output["permissionDecision"], "allow", "{tool}");
        if tool == "Bash" {
            assert_eq!(fixture.permission(output["updatedInput"].clone()), "allow");
            assert_eq!(fixture.permission(output["updatedInput"].clone()), "deny");
        }
    }
    for tool in [
        "Agent",
        "Task",
        "TaskOutput",
        "TaskStop",
        "CronCreate",
        "CronDelete",
        "CronList",
    ] {
        let mut fixture = Fixture::with_extensions(None, Uuid::new_v4().to_string(), |config| {
            config.sandbox = Sandbox::FullAccess;
        });
        fixture.turn();
        assert_eq!(
            fixture.hook(tool, json!({}))["permissionDecision"],
            "deny",
            "{tool}"
        );
    }
}

#[test]
fn assistant_split_frames_keep_raw_order_and_summarize_only_at_boundaries() {
    let mut f = Fixture::new();
    f.turn();
    f.assistant(
        Some("native-1"),
        "claude-sonnet-5",
        json!([
            {"type":"text","text":"first"},
            {"type":"tool_use","id":"read-1","name":"Read","input":{"file_path":"foo"}}
        ]),
    );
    assert_eq!(f.codec.pop().unwrap()["params"]["item"]["text"], "first");
    assert_eq!(
        f.codec.pop().unwrap()["params"]["item"]["type"],
        "dynamicToolCall"
    );
    f.assistant(
        None,
        "<synthetic>",
        json!([{"type":"text","text":"local notice"}]),
    );
    assert_eq!(
        f.codec.pop().unwrap()["params"]["item"]["verifiedModel"],
        false
    );
    f.assistant(
        Some("native-1"),
        "claude-sonnet-5",
        json!([
            {"type":"thinking","thinking":"not output"},{"type":"text","text":"second"}
        ]),
    );
    assert_eq!(f.codec.pop().unwrap()["params"]["item"]["text"], "second");
    assert!(f.codec.pop().is_none());
    f.assistant(
        Some("native-2"),
        "claude-sonnet-5",
        json!([{"type":"text","text":"new"}]),
    );
    let summary = f.codec.pop().unwrap();
    assert_eq!(summary["params"]["item"]["text"], "first\nsecond");
    assert_eq!(summary["params"]["item"]["aggregation"], "native_message");
    assert_eq!(summary["params"]["item"]["nativeMessageId"], "native-1");
    assert_eq!(f.codec.pop().unwrap()["params"]["item"]["text"], "new");
    f.assistant(
        None,
        "claude-sonnet-5",
        json!([{"type":"text","text":"idless-a"}]),
    );
    f.assistant(
        None,
        "claude-sonnet-5",
        json!([{"type":"text","text":"idless-b"}]),
    );
    for text in ["idless-a", "idless-b"] {
        assert_eq!(f.codec.pop().unwrap()["params"]["item"]["text"], text);
    }
    assert!(f.codec.pop().is_none());
}

#[test]
fn assistant_aggregate_bound_fails_explicitly_without_cumulative_summaries() {
    let mut f = Fixture::new();
    f.turn();
    let large = "x".repeat(1024 * 1024);
    for _ in 0..3 {
        f.assistant(
            Some("large"),
            "claude-sonnet-5",
            json!([{"type":"text","text":large}]),
        );
        let raw = f.codec.pop().unwrap();
        assert_eq!(
            raw["params"]["item"]["text"].as_str().unwrap().len(),
            large.len()
        );
        assert!(raw["params"]["item"].get("aggregation").is_none());
        assert!(f.codec.pop().is_none());
    }
    let error = f
        .codec
        .receive(json!({"type":"assistant","session_id":f.id,"message":{
        "id":"large","model":"claude-sonnet-5","content":[{"type":"text","text":large}]}}))
        .unwrap_err();
    assert_eq!(error.code(), "output_limit");
    assert!(f.codec.pop().is_none());
}

#[test]
fn shell_permission_requires_exact_issued_input_once() {
    let mut f = Fixture::new();
    f.turn();
    assert_eq!(f.permission(json!({"command":"echo hi"})), "deny");
    let output = f.hook("Bash", json!({"command":"echo hi"}));
    assert_eq!(output["permissionDecision"], "allow");
    let allowed = output["updatedInput"].clone();
    let mut forged = allowed.clone();
    forged["command"] = json!(format!(
        "{}; echo extra",
        forged["command"].as_str().unwrap()
    ));
    assert_eq!(f.permission(forged), "deny");
    {
        let id = json!("other-tool");
        let reply = f.codec.receive(json!({"type":"control_request","request_id":"bad-id",
            "request":{"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":id,"input":allowed}})).unwrap();
        assert_eq!(reply[0]["response"]["response"]["behavior"], "deny");
    }
    assert_eq!(f.permission(allowed.clone()), "allow");
    assert_eq!(f.permission(allowed), "deny");
}

#[test]
fn read_only_bash_runs_only_the_rendered_git_command_once() {
    let mut f = Fixture::with_extensions(None, Uuid::new_v4().to_string(), |config| {
        config.sandbox = Sandbox::ReadOnly;
    });
    f.turn();
    for refused in [
        json!({"command":"echo hi"}),
        json!({"command":"git status; touch pwned"}),
        json!({"command":"git status","run_in_background":true}),
        json!({"command":"git status","dangerouslyDisableSandbox":true}),
    ] {
        let output = f.hook("Bash", refused.clone());
        assert_eq!(output["permissionDecision"], "deny", "{refused}");
        assert_eq!(
            output["permissionDecisionReason"],
            "Read-only agents may run one allowlisted read-only git command without shell syntax."
        );
    }
    let output = f.hook("Bash", json!({"command":"git status --porcelain"}));
    assert_eq!(output["permissionDecision"], "allow");
    let allowed = output["updatedInput"].clone();
    let command = allowed["command"].as_str().unwrap();
    // The agent's text never reaches the shell: only the checked rendering does.
    assert!(
        command.ends_with(
            "\nGIT_OPTIONAL_LOCKS=0 GIT_PAGER=cat PAGER=cat GIT_TERMINAL_PROMPT=0 \
             GIT_NO_LAZY_FETCH=1 exec git --no-pager -c core.fsmonitor=false 'status' '--porcelain'"
        ),
        "{command}"
    );
    assert_eq!(
        f.permission(json!({"command":"git status --porcelain"})),
        "deny"
    );
    assert_eq!(f.permission(allowed.clone()), "allow");
    assert_eq!(f.permission(allowed), "deny");
}

#[test]
fn file_policy_rejects_case_variants_symlinks_and_dual_notebook_paths() {
    for path in [
        ".GIT/config",
        ".CLAUDE/settings.json",
        ".Codex/config.toml",
        ".huſ\u{212a}y/hook",
        ".proﬁle",
        ".mcp.jſon",
        "../escaped",
        "nested/.Git/hooks/x",
    ] {
        let mut f = Fixture::new();
        f.turn();
        let file = f.dir.path().canonicalize().unwrap().join(path);
        assert_eq!(
            f.hook("Write", json!({"file_path":file}))["permissionDecision"],
            "deny"
        );
    }
    let mut f = Fixture::new();
    f.turn();
    let good = f.dir.path().canonicalize().unwrap().join("good.ipynb");
    assert_eq!(
        f.hook(
            "NotebookEdit",
            json!({"file_path":good,"notebook_path":"/outside.ipynb"})
        )["permissionDecision"],
        "deny"
    );
    std::os::unix::fs::symlink("/tmp", f.dir.path().join("link")).unwrap();
    assert_eq!(
        f.hook(
            "Write",
            json!({"file_path":f.dir.path().canonicalize().unwrap().join("link/out")})
        )["permissionDecision"],
        "deny"
    );
    std::fs::create_dir(f.dir.path().join("ordinary")).unwrap();
    std::os::unix::fs::symlink("ordinary", f.dir.path().join(".git")).unwrap();
    assert_eq!(
        f.hook(
            "Write",
            json!({"file_path":f.dir.path().canonicalize().unwrap().join("ordinary/config")})
        )["permissionDecision"],
        "deny"
    );
}

#[test]
fn launch_rejects_protected_workspaces_and_gates_readonly_bash() {
    let launcher = crate::launch::test_cache().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut config = SessionConfig::for_test(
        Uuid::new_v4().to_string(),
        dir.path().canonicalize().unwrap(),
    );
    let mut command = std::process::Command::new("unused");
    let _guard = config.configure(&mut command, &launcher).unwrap();
    let args: Vec<_> = command.get_args().map(|s| s.to_str().unwrap()).collect();
    let tools = args[args.iter().position(|a| *a == "--tools").unwrap() + 1];
    // Read-only agents get Bash only for checked read-only git commands.
    assert_eq!(tools, "Bash,Read,Glob,Grep");
    for name in [".git", ".CLAUDE", ".huſ\u{212a}y", "bracket[dir]"] {
        let path = dir.path().join(name);
        std::fs::create_dir(&path).unwrap();
        config.cwd = path.canonicalize().unwrap();
        let error = config
            .configure(&mut std::process::Command::new("unused"), &launcher)
            .err()
            .unwrap();
        assert_eq!(error.code(), "invalid_path");
    }
    config.cwd = dir.path().canonicalize().unwrap();
    config.sandbox = Sandbox::WorkspaceWrite;
    let mut command = std::process::Command::new("unused");
    let _guard = config.configure(&mut command, &launcher).unwrap();
    let args: Vec<_> = command.get_args().map(|s| s.to_str().unwrap()).collect();
    let settings: Value =
        serde_json::from_str(args[args.iter().position(|a| *a == "--settings").unwrap() + 1])
            .unwrap();
    let deny = settings["sandbox"]["filesystem"]["denyWrite"]
        .as_array()
        .unwrap();
    assert!(deny.contains(&json!(config.cwd.join(".git"))));
    assert!(deny.contains(&json!(launcher.protected_root().unwrap())));
    #[cfg(target_os = "macos")]
    for pattern in [
        ".[hH][uU]ſ\u{212a}[yY]",
        ".[pP][rR][oO]ﬁ[lL][eE]",
        ".[mM][cC][pP].[jJ]ſ[oO][nN]",
    ] {
        assert!(deny.contains(&json!(config.cwd.join(pattern))), "{deny:?}");
    }
}

#[test]
fn echo_ledger_overflow_keeps_prior_echoes_and_fails_explicitly() {
    let mut f = Fixture::new();
    for id in 0..256 {
        f.codec
            .receive(
                json!({"type":"control_request","request_id":format!("ping-{id}"),
            "request":{"subtype":"mcp_message","server_name":"hive",
            "message":{"jsonrpc":"2.0","id":id,"method":"ping"}}}),
            )
            .unwrap();
    }
    let extra = f
        .codec
        .receive(json!({"type":"control_request","request_id":"overflow",
        "request":{"subtype":"mcp_message","server_name":"hive",
        "message":{"jsonrpc":"2.0","id":256,"method":"ping"}}}))
        .unwrap_err();
    assert_eq!(extra.code(), "output_limit");
    let echo = json!({"type":"control_response","response":{"subtype":"success","request_id":"ping-0",
        "response":{"mcp_response":{"jsonrpc":"2.0","id":0,"result":{}}}}});
    assert!(f.codec.receive(echo).unwrap().is_empty());
}

#[test]
#[cfg(target_os = "linux")] // APFS refuses non-UTF8 names at creation.
fn non_utf8_protected_symlink_targets_fail_without_panicking() {
    use std::os::unix::ffi::OsStringExt;
    let launcher = crate::launch::test_cache().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let target = dir
        .path()
        .join(std::ffi::OsString::from_vec(vec![b'x', 0xff]));
    std::fs::create_dir(&target).unwrap();
    std::os::unix::fs::symlink(&target, dir.path().join(".git")).unwrap();
    let config = SessionConfig {
        sandbox: Sandbox::WorkspaceWrite,
        ..SessionConfig::for_test(
            Uuid::new_v4().to_string(),
            dir.path().canonicalize().unwrap(),
        )
    };
    let error = config
        .configure(&mut std::process::Command::new("unused"), &launcher)
        .err()
        .unwrap();
    assert_eq!(error.code(), "invalid_path");
}

#[test]
fn acknowledged_input_must_not_reappear_in_interrupt_cancellation_queue() {
    let mut f = Fixture::new();
    let turn = f.turn();
    let request = f
        .codec
        .send(outgoing::request(4, outgoing::interrupt(&f.id, &turn)))
        .unwrap();
    assert!(
        f.codec
            .receive(json!({"type":"control_response","response":{
        "subtype":"success","request_id":request[0]["request_id"],
        "response":{"cancelled":[turn],"still_queued":[]}}}))
            .is_err()
    );
}

#[test]
fn open_claude_wire_tags_classify_unknown_strings_without_accepting_other_shapes() {
    macro_rules! check {
        ($kind:ty, $known:expr, $variant:expr, $other:expr) => {{
            assert_eq!(serde_json::from_value::<$kind>(json!($known)).unwrap(), $variant);
            for unknown in ["future", ""] {
                assert_eq!(serde_json::from_value::<$kind>(json!(unknown)).unwrap(), $other);
            }
            for malformed in [json!(null), json!(7), json!([]), json!({$known:null})] {
                assert!(serde_json::from_value::<$kind>(malformed).is_err());
            }
        }};
    }
    check!(
        super::super::wire::ResultSubtype,
        "success",
        super::super::wire::ResultSubtype::Success,
        super::super::wire::ResultSubtype::Other
    );
    check!(
        super::super::wire::TerminalReason,
        "completed",
        super::super::wire::TerminalReason::Completed,
        super::super::wire::TerminalReason::Other
    );
    check!(
        super::super::wire::SystemSubtype,
        "init",
        super::super::wire::SystemSubtype::Init,
        super::super::wire::SystemSubtype::Other
    );
    check!(
        super::super::wire::OriginKind,
        "human",
        super::super::wire::OriginKind::Human,
        super::super::wire::OriginKind::Other
    );
    check!(
        super::super::wire::HookEvent,
        "PostToolUse",
        super::super::wire::HookEvent::PostToolUse,
        super::super::wire::HookEvent::Other
    );
    check!(
        super::super::wire::CallbackId,
        "hive-guard",
        super::super::wire::CallbackId::Guard,
        super::super::wire::CallbackId::Other
    );
}
