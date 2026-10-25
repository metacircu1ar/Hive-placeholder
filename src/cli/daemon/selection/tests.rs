use super::*;
use crate::cli::response_tests::Stub;
use serde_json::{Value, json};

fn fixture() -> (Expected, Value) {
    let expected = Expected {
        binary_sha256: Some("d".repeat(64)),
        configuration: ConfigurationIdentity {
            path: "/private/config.yaml".into(),
            sha256: "a".repeat(64),
        },
        providers: Some(runtime::ProviderSelection {
            codex: "b".repeat(64),
            claude: "c".repeat(64),
        }),
        label: None,
    };
    let value = json!({"binary_sha256":"d".repeat(64),"protocols":{"minimum":1,"maximum":1},"configuration":expected.configuration, "provider_selection":expected.providers,
        "status":"running", "pid":123});
    (expected, value)
}

#[test]
fn daemon_start_matches_selection_and_rejects_incomplete_metadata() {
    let (expected, original) = fixture();
    assert!(expected.check(original.clone()).unwrap().success);
    for field in ["configuration", "provider_selection", "status"] {
        let mut missing = original.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert_eq!(
            expected.check(missing).err().unwrap().code(),
            "daemon_protocol_error"
        );
    }
    for (pointer, replacement, field) in [
        (
            "/configuration/path",
            json!("/another/config.yaml"),
            "--config",
        ),
        ("/configuration/sha256", json!("d".repeat(64)), "--config"),
        (
            "/provider_selection/codex",
            json!("d".repeat(64)),
            "--codex-bin",
        ),
        (
            "/provider_selection/claude",
            json!("d".repeat(64)),
            "--claude-bin",
        ),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(pointer).unwrap() = replacement;
        let Error::Input { input, .. } = expected.check(changed).err().unwrap() else {
            panic!("input error")
        };
        assert_eq!(input.field(), field);
    }
    for (pointer, replacement) in [
        ("/provider_selection/codex", json!("not-a-digest")),
        (
            "/provider_selection",
            json!(["b".repeat(64), "c".repeat(64)]),
        ),
        (
            "/configuration",
            json!(["/private/config.yaml", "a".repeat(64)]),
        ),
        ("/status", json!({"running":null})),
        ("/status", json!("stopping")),
    ] {
        let mut invalid = original.clone();
        *invalid.pointer_mut(pointer).unwrap() = replacement;
        assert_eq!(
            expected.check(invalid).err().unwrap().code(),
            "daemon_protocol_error"
        );
    }
}

#[test]
fn daemon_start_readiness_race_cannot_report_a_different_owner_as_success() {
    let (expected, mut racing) = fixture();
    racing["provider_selection"]["claude"] = json!("d".repeat(64));
    let stub = Stub::new(vec![racing]);
    let mut child = crate::test_support::ChildGuard(
        crate::os::spawn(std::process::Command::new("/bin/sleep").arg("10")).unwrap(),
    );
    let result = super::super::wait_for_daemon(
        stub.root(),
        &mut child,
        &expected,
        Instant::now() + std::time::Duration::from_secs(1),
    );
    assert!(child.try_wait().unwrap().is_none());
    assert_eq!(result.err().unwrap().code(), "conflicting_arguments");
    assert_eq!(stub.finish().len(), 1);
}

#[test]
fn daemon_start_wire_failure_is_returned_without_spawning_or_reclassification() {
    let (expected, _) = fixture();
    let wire = json!({"error":{"code":"daemon_unavailable","message":"server unavailable"}});
    let stub = Stub::new(vec![wire.clone()]);
    let outcome = expected
        .observe(
            stub.root(),
            Instant::now() + std::time::Duration::from_secs(1),
        )
        .unwrap()
        .unwrap();
    assert!(!outcome.success);
    assert_eq!(outcome.value, wire);
    assert_eq!(stub.finish().len(), 1);
}

#[test]
fn daemon_start_socket_close_before_child_exit_keeps_log_guidance_and_cause() {
    use std::error::Error as _;
    let (expected, _) = fixture();
    let tmp = tempfile::tempdir_in("/tmp").unwrap();
    std::fs::create_dir(tmp.path().join("runtime")).unwrap();
    let script = r"
import socket, sys, time
listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
listener.bind(sys.argv[1])
listener.listen(1)
connection, _ = listener.accept()
connection.recv(65536)
connection.close()
listener.close()
time.sleep(0.3)
sys.exit(7)
";
    let mut child = crate::test_support::ChildGuard(
        crate::os::spawn(
            std::process::Command::new("python3")
                .args(["-c", script])
                .arg(runtime::socket_path(tmp.path())),
        )
        .unwrap(),
    );
    let error = super::super::wait_for_daemon(
        tmp.path(),
        &mut child,
        &expected,
        Instant::now() + std::time::Duration::from_secs(3),
    )
    .err()
    .unwrap();
    assert_eq!(error.code(), "daemon_start_failed");
    assert!(error.message().contains("runtime/daemon.log"));
    assert!(error.message().contains("status 7"));
    assert!(error.message().contains("runtime/logs/hive-daemon.jsonl"));
    let cause = error.source().unwrap().downcast_ref::<Error>().unwrap();
    assert!(matches!(cause.code(), "daemon_protocol_error" | "state_io"));
    assert!(cause.source().is_some());
    assert_eq!(child.try_wait().unwrap().unwrap().code(), Some(7));
}

#[test]
fn daemon_selection_refuses_changed_images_and_disjoint_protocols() {
    let (expected, original) = fixture();
    let mut changed = original.clone();
    changed["binary_sha256"] = json!("e".repeat(64));
    assert_eq!(
        expected.check(changed).err().unwrap().code(),
        "daemon_binary_mismatch"
    );
    let mut future = original;
    future["protocols"] = json!({"minimum":2,"maximum":2});
    assert_eq!(
        expected.check(future).err().unwrap().code(),
        "incompatible_protocol"
    );
}
