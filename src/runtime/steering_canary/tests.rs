#![allow(clippy::unwrap_used)]
use super::*;

#[derive(Clone, Copy)]
enum CanaryCase {
    Echo,
    Continuation,
    Missing,
    Synthetic,
    Cancelled,
}
impl CanaryCase {
    fn wire(self) -> &'static str {
        match self {
            Self::Echo => "echo",
            Self::Continuation => "continuation",
            Self::Missing => "missing",
            Self::Synthetic => "synthetic",
            Self::Cancelled => "cancelled",
        }
    }
}

#[test]
fn canary_requires_hook_only_nonce_in_verified_same_turn_final_output() {
    for scenario in [
        CanaryCase::Echo,
        CanaryCase::Continuation,
        CanaryCase::Missing,
        CanaryCase::Synthetic,
        CanaryCase::Cancelled,
    ] {
        let case = scenario.wire();
        let dir = super::super::test_directory();
        let cwd = dir.path().canonicalize().unwrap();
        std::fs::write(cwd.join("fixed.txt"), "fixed harmless content").unwrap();
        let session = uuid::Uuid::new_v4().to_string();
        let config = Arc::new(SessionConfig {
            instructions: "Read the fixed file once".into(),
            steering: Some(SteeringPermit::Canary),
            ..SessionConfig::for_test(session.clone(), cwd.clone())
        });
        let mut command = std::process::Command::new(
            crate::process::executable(std::ffi::OsStr::new("python3")).unwrap(),
        );
        command
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude_canary.py"))
            .arg(case);
        let launcher = crate::launch::test_cache().unwrap();
        let mut rpc =
            Rpc::start_authorized(&launcher, command, Backend::Claude(config), vec![], |_| {
                Ok(())
            })
            .unwrap();
        rpc.capture_limit(4 * 1024 * 1024);
        rpc.initialize().unwrap();
        rpc.call(outgoing::thread_start(None, None)).unwrap();
        let cancellation = Cancellation::default();
        if matches!(scenario, CanaryCase::Cancelled) {
            cancellation.request();
        }
        let result = run(&mut rpc, &session, &cwd, &cancellation);
        let shutdown = rpc.close();
        assert!(
            shutdown.confirmed,
            "case {case}: {}",
            shutdown.method.as_str()
        );
        match scenario {
            CanaryCase::Echo | CanaryCase::Continuation => {
                let evidence = result.unwrap();
                assert!(evidence.output.contains(&evidence.nonce));
                assert_eq!(evidence.receipt.turn(), evidence.turn);
                assert_eq!(
                    evidence.mode,
                    if matches!(scenario, CanaryCase::Echo) {
                        provider::claude::ReceiptMode::NativeEcho
                    } else {
                        provider::claude::ReceiptMode::WrittenContinuation
                    }
                );
            }
            CanaryCase::Cancelled => assert!(matches!(result, Err(Error::Cancelled { .. }))),
            CanaryCase::Missing | CanaryCase::Synthetic => {
                assert!(
                    matches!(result, Err(Error::UnsupportedOperation { .. })),
                    "case {case}"
                );
            }
        }
    }
}
#[test]
fn canary_cannot_bypass_daemon_maintenance_ownership() {
    let root = super::super::test_directory();
    drop(
        State::new(root.path().into())
            .unwrap()
            .initialize()
            .unwrap(),
    );
    state::directory(&root.path().join("runtime")).unwrap();
    let _owner = state::Lock::acquire(&root.path().join("runtime/daemon.lock")).unwrap();
    let config = claude_config();
    let result = verify(
        &config,
        &State::new(root.path().into()).unwrap(),
        "claude-main",
        "sonnet",
        "low",
        Path::new("/not-launched"),
    );
    assert!(matches!(result, Err(Error::DaemonBusy { .. })));
}

#[test]
fn cancellation_during_native_settings_preserves_proof_and_never_submits_inference() {
    let (root, binary, old) = proof::tests::fixture();
    proof::write(&proof::path(root.path()), Some(old.clone())).unwrap();
    let state = State::new(root.path().into()).unwrap();
    let config = claude_config();
    let binding = state
        .register("claude-main", config.profile("claude-main").unwrap())
        .unwrap();
    let owner = super::super::maintenance_lock(root.path()).unwrap();
    let session = uuid::Uuid::new_v4().to_string();
    let cwd = root.path().canonicalize().unwrap();
    let config = Arc::new(SessionConfig {
        instructions: "Read once".into(),
        steering: Some(SteeringPermit::Canary),
        ..SessionConfig::for_test(session.clone(), cwd.clone())
    });
    let mut command = std::process::Command::new(
        crate::process::executable(std::ffi::OsStr::new("python3")).unwrap(),
    );
    command
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude_canary.py"))
        .arg("cancel-preflight");
    let launcher = crate::launch::test_cache().unwrap();
    let mut rpc =
        Rpc::start_authorized(&launcher, command, Backend::Claude(config), vec![], |_| {
            Ok(())
        })
        .unwrap();
    let cancellation = Cancellation::default();
    let signal = cancellation.clone();
    let path = cwd.clone();
    let (exercise_done, done) = std::sync::mpsc::channel::<()>();
    let cancelling = std::thread::spawn(move || {
        // Follow the bounded native preflight, including cold process startup.
        // An early error ends this observer and is reported by the main thread.
        while !path.join("settings-waiting").exists() {
            if done.recv_timeout(Duration::from_millis(5))
                != Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            {
                return false;
            }
        }
        signal.request();
        std::fs::write(path.join("release-settings"), "").unwrap();
        true
    });
    let canary = Canary {
        launcher: &launcher,
        state: &state,
        binding: &binding,
        binary: &binary,
        identity: &old.binary,
        digest: &old.digest,
        model: "sonnet",
        effort: "low",
        cwd: &cwd,
        owner: &owner,
        cancellation: &cancellation,
        old: Some(&old),
    };
    let result = canary.exercise(&mut rpc, session, "2.1.test".into());
    drop(exercise_done);
    assert!(
        cancelling.join().unwrap(),
        "preflight ended before settings handshake: {:?}",
        result.as_ref().err()
    );
    assert!(
        matches!(result, Err(Error::Cancelled { .. })),
        "{:?}",
        result.err()
    );
    assert!(rpc.close().confirmed);
    assert!(!cwd.join("inference-submitted").exists());
    let retained = proof::read(&proof::path(root.path())).unwrap().unwrap();
    assert_eq!(retained.nonce_sha256, old.nonce_sha256);
}

fn claude_config() -> Config {
    Config::parse("auth_profiles: {claude-main: {provider: claude, method: subscription}}").unwrap()
}
