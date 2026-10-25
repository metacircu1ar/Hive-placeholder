//! Lifecycle regressions.
use super::*;

#[test]
fn fast_missing_resume_is_drained_before_broken_pipe_is_classified() {
    let launcher = crate::launch::test_cache().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let config = Arc::new(SessionConfig {
        history: crate::runtime::provider::claude::History::Resume(
            crate::runtime::provider::claude::IdentityPrompt::NativeSnapshot,
        ),
        sandbox: Sandbox::WorkspaceWrite,
        ..SessionConfig::for_test(
            Uuid::new_v4().to_string(),
            dir.path().canonicalize().unwrap(),
        )
    });
    let mut command = Command::new("/bin/sh");
    command.args(["-c","exec 0<&-; printf '%s\\n' '{\"type\":\"result\",\"session_id\":\"startup\",\"subtype\":\"error_during_execution\",\"num_turns\":0,\"is_error\":true}'; exit 1"]);
    let mut rpc = super::super::super::rpc::Rpc::start_with(
        &launcher,
        command,
        super::super::super::Backend::Claude(config),
        vec![],
    )
    .unwrap();
    // Ensure the launch rejection precedes the first control write.
    std::thread::sleep(Duration::from_millis(100));
    assert!(rpc.initialize().is_err());
    assert!(!rpc.uncertain());
    assert!(rpc.close().confirmed);
}
