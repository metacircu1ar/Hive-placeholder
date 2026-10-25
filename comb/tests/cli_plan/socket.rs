use super::*;
use std::{fs, os::unix::fs::PermissionsExt};

#[test]
fn socket_privacy_errors_precede_managed_state_and_setup_effects() {
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let parent = temp.path().join("public");
    fs::create_dir(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
    for setup in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_comb"));
        command
            .arg("--root")
            .arg(temp.path().join("missing-root"))
            .arg("--socket")
            .arg(parent.join("s.sock"));
        if setup {
            command.args(["setup", "--no-services"]);
        } else {
            command
                .arg("--managed-service")
                .arg(temp.path().join("missing-spec"))
                .arg("serve");
        }
        let output = command.stdin(Stdio::null()).output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(2),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let reply: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(reply["error"]["field"], "--socket");
        assert_eq!(reply["error"]["code"], "invalid_path");
        assert_eq!(fs::read_dir(&parent).unwrap().count(), 0);
        assert!(!temp.path().join("missing-root").exists());
    }
}

#[test]
fn world_writable_socket_route_is_refused_without_a_chmod_remedy() {
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let ancestor = temp.path().join("group\nlayout");
    let parent = ancestor.join("private");
    fs::create_dir_all(&parent).unwrap();
    fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o777)).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
    for setup in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_comb"));
        command
            .arg("--root")
            .arg(temp.path().join("missing-root"))
            .arg("--socket")
            .arg(parent.join("s.sock"));
        if setup {
            command.args(["setup", "--no-services"]);
        } else {
            command.arg("serve");
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains(r"group\nlayout"), "{stderr}");
        assert!(!stderr.contains("chmod g-w"), "{stderr}");
        let stdout = String::from_utf8(output.stdout).unwrap();
        let reply: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(reply["error"]["field"], "--socket");
        assert!(!stdout.contains("layout"));
        assert!(!stdout.contains("chmod"));
        assert!(!temp.path().join("missing-root").exists());
        assert_eq!(fs::read_dir(&parent).unwrap().count(), 0);
    }
}
