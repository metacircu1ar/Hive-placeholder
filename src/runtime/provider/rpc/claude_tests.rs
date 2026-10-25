//! Regressions that need the RPC's buffered native transport state.
use super::*;
use crate::runtime::provider::claude::SessionConfig;
use std::sync::Arc;

#[test]
fn exited_claude_metadata_budget_cannot_hide_execution_evidence() {
    let launcher = crate::launch::test_cache().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let config = Arc::new(SessionConfig::for_test(
        uuid::Uuid::new_v4().to_string(),
        dir.path().canonicalize().unwrap(),
    ));
    let mut command = Command::new("/bin/sh");
    command.args(["-c", r#"
        IFS= read -r request
        printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"hive-1","response":{"models":[],"account":{"apiProvider":"firstParty"}}}}'
        cat >/dev/null
    "#]);
    let mut rpc = Rpc::start_with(
        &launcher,
        command,
        super::super::Backend::Claude(config),
        vec![],
    )
    .unwrap();
    rpc.initialize().unwrap();
    rpc.pipes.child.stdin.take();
    crate::test_support::wait_until(Duration::from_secs(5), Duration::from_millis(10), || {
        process::has_exited(&rpc.pipes.child).unwrap().then_some(())
    });
    // Model a coalesced final pipe read already buffered when the process exits.
    // The first read_next budget ends immediately before execution evidence.
    for id in 0..256 {
        let frame = serde_json::json!({"type":"control_request","request_id":format!("native-{id}"),
            "request":{"subtype":"mcp_message","server_name":"hive","message":{
                "jsonrpc":"2.0","id":id,"method":"ping"}}});
        rpc.pipes.buffer.extend(serde_json::to_vec(&frame).unwrap());
        rpc.pipes.buffer.push(b'\n');
    }
    rpc.pipes
        .buffer
        .extend(b"{\"type\":\"system\",\"subtype\":\"task_started\"}\n");
    let report = rpc.close();
    assert!(!report.confirmed);
    assert!(!report.quiescent);
    assert!(report.reaped);
}

#[test]
fn rejected_outbox_batch_is_atomic_and_uncertain_flush_discards_it() {
    let launcher = crate::launch::test_cache().unwrap();
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "cat >/dev/null"]);
    let mut rpc =
        Rpc::start_with(&launcher, command, super::super::Backend::Codex, vec![]).unwrap();
    let first = serde_json::json!({"id":1});
    rpc.queue_output(vec![first.clone()]).unwrap();
    let error = rpc
        .queue_output(vec![Value::Null; PENDING_FRAME_LIMIT])
        .unwrap_err();
    assert_eq!(error.code(), "output_limit");
    assert_eq!(rpc.outbox.frames.iter().collect::<Vec<_>>(), vec![&first]);
    rpc.protocol_uncertain = true;
    assert!(rpc.flush_output().is_err());
    assert!(rpc.outbox.frames.is_empty());
    rpc.close();
}

#[test]
fn uncertain_active_claude_shutdown_skips_eof_grace() {
    let launcher = crate::launch::test_cache().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let config = Arc::new(SessionConfig::for_test(
        uuid::Uuid::new_v4().to_string(),
        dir.path().canonicalize().unwrap(),
    ));
    // Sleep never reads stdin; EOF cannot make it exit. TERM should arrive
    // promptly instead of waiting the ten-second EOF grace on a broken channel.
    // The Claude launch appends its own flags, which sleep would reject at once
    // with exit 1; through sh they are ignored positional parameters.
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "exec /bin/sleep 60", "sleep-fixture"]);
    let mut rpc = Rpc::start_with(
        &launcher,
        command,
        super::super::Backend::Claude(config),
        vec![],
    )
    .unwrap();
    rpc.threads.entry("thread".into()).or_default().turn = Some(Turn::Active("turn".into()));
    rpc.poison();
    let started = Instant::now();
    let report = rpc.close();
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(
        report.method,
        crate::runtime::provider::ShutdownMethod::Sigterm
    );
    assert!(report.reaped);
    assert!(!report.confirmed);
}

#[test]
fn claude_shutdown_waits_for_group_absence_but_never_confirms_a_live_group() {
    // The orphaned background job must come back to this process to be reaped,
    // as it does to the daemon; adoption is process-wide, so run isolated.
    crate::test_support::run_isolated(&mut crate::test_support::rerun_isolated(
        "runtime::provider::rpc::claude_tests::group_absence_fixture",
        true,
    ));
}
#[test]
#[ignore = "isolated fixture: adopts orphans like the daemon"]
fn group_absence_fixture() {
    #[cfg(target_os = "linux")]
    crate::os::adopt_orphans();
    let launcher = crate::launch::test_cache().unwrap();
    for (delay, confirmed) in [("0.4", true), ("gated", false)] {
        let dir = tempfile::tempdir().unwrap();
        let config = Arc::new(SessionConfig::for_test(
            uuid::Uuid::new_v4().to_string(),
            dir.path().canonicalize().unwrap(),
        ));
        let mut command = Command::new("/bin/sh");
        let release = dir.path().join("release");
        command.env("HIVE_TEST_HELPER_DELAY", delay)
            .env("HIVE_TEST_HELPER_RELEASE", &release).args(["-c", r#"
            IFS= read -r request
            printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"hive-1","response":{"models":[],"account":{"apiProvider":"firstParty"}}}}'
            cat >/dev/null
            if [ "$HIVE_TEST_HELPER_DELAY" = gated ]; then
                (
                    attempts=0
                    while [ ! -f "$HIVE_TEST_HELPER_RELEASE" ] && [ "$attempts" -lt 3000 ]; do
                        sleep 0.02
                        attempts=$((attempts + 1))
                    done
                ) </dev/null >/dev/null 2>&1 &
            else
                sleep "$HIVE_TEST_HELPER_DELAY" </dev/null >/dev/null 2>&1 &
            fi
        "#]);
        let mut rpc = Rpc::start_with(
            &launcher,
            command,
            super::super::Backend::Claude(config),
            vec![],
        )
        .unwrap();
        rpc.initialize().unwrap();
        let report = rpc.close();
        std::fs::write(release, b"release").unwrap();
        // Release the negative case only after the result, avoiding reliance
        // on scheduling speed. Its bounded fallback also covers test crashes.
        assert!(process::wait_group_absent(rpc.pipes.child.id()).unwrap());
        assert_eq!(report.confirmed, confirmed);
        assert_eq!(report.method, crate::runtime::provider::ShutdownMethod::Eof);
        assert_eq!(report.exit_code, Some(0));
        assert!(report.reaped);
    }
}
