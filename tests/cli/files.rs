//! Content validation occurs before defaults, managed setup, sockets and progress.
use super::support;
use serde_json::Value;
use std::{path::Path, process::Command};

fn command(root: &Path, args: &[&str]) -> Command {
    let mut command = support::command(env!("CARGO_BIN_EXE_hivectl"));
    command
        .current_dir(root)
        .env_remove("HOME")
        .env_remove("XDG_STATE_HOME")
        .env_remove("XDG_CONFIG_HOME")
        .args(args);
    command
}

fn rejection(root: &Path, args: &[&str], code: &str, flag: &str, path: bool) {
    let output = command(root, args).output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["code"], code, "{args:?}");
    assert_eq!(value["error"]["field"], flag, "{args:?}");
    let human = String::from_utf8(output.stderr).unwrap();
    assert_eq!(human.contains("Path:"), path);
    assert!(!value.to_string().contains("PRIVATE"));
    assert!(!human.contains("SECRET_CONTENT"));
    assert!(!human.contains("request ID"));
    assert!(!human.contains("HOME"));
    assert!(!human.contains("Managed execution"));
    for example in value["error"]["examples"].as_array().unwrap() {
        assert!(human.contains(example.as_str().unwrap()));
    }
}

#[test]
fn selected_files_fail_with_flag_and_human_path_before_defaults_or_startup() {
    let temp = tempfile::tempdir().unwrap();
    for (args, flag) in [
        (
            vec!["spawn", "--cwd", "/tmp", "--body-file", "PRIVATE"],
            "--body-file",
        ),
        (
            vec![
                "spawn",
                "--cwd",
                "/tmp",
                "--body",
                "work",
                "--instructions-file",
                "PRIVATE",
            ],
            "--instructions-file",
        ),
        (
            vec!["send", "a", "--body-file", "PRIVATE", "--wait"],
            "--body-file",
        ),
        (
            vec!["complete", "a", "--result-file", "PRIVATE"],
            "--result-file",
        ),
        (
            vec!["batch", "--operations-file", "PRIVATE"],
            "--operations-file",
        ),
        (
            vec![
                "respond",
                "00000000-0000-4000-8000-000000000001",
                "--accept",
                "--data-file",
                "PRIVATE",
            ],
            "--data-file",
        ),
    ] {
        rejection(temp.path(), &args, "invalid_file", flag, true);
    }
    #[cfg(feature = "comb")]
    rejection(
        temp.path(),
        &["evidence", "correction", "--summary-file", "PRIVATE"],
        "invalid_file",
        "--summary-file",
        true,
    );
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[test]
fn invalid_file_contents_and_nonregular_sources_are_structured_and_private() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("PRIVATE");
    std::fs::write(&file, b"SECRET_CONTENT\xff").unwrap();
    rejection(
        temp.path(),
        &["send", "a", "--body-file", "PRIVATE", "--wait"],
        "invalid_file",
        "--body-file",
        true,
    );
    std::fs::write(&file, "SECRET_CONTENT").unwrap();
    rejection(
        temp.path(),
        &["batch", "--operations-file", "PRIVATE"],
        "invalid_json",
        "--operations-file",
        true,
    );
    std::fs::write(&file, "{}").unwrap();
    rejection(
        temp.path(),
        &["batch", "--operations-file", "PRIVATE"],
        "invalid_json",
        "--operations-file",
        true,
    );
    std::fs::write(&file, vec![b'a'; 65537]).unwrap();
    rejection(
        temp.path(),
        &["send", "a", "--body-file", "PRIVATE"],
        "out_of_range",
        "--body-file",
        true,
    );
    #[cfg(feature = "comb")]
    {
        std::fs::write(&file, vec![b'a'; 8193]).unwrap();
        rejection(
            temp.path(),
            &["evidence", "correction", "--summary-file", "PRIVATE"],
            "out_of_range",
            "--summary-file",
            true,
        );
        rejection(
            temp.path(),
            &["evidence", "correction", "--summary", &"a".repeat(8193)],
            "out_of_range",
            "--summary",
            false,
        );
    }
    for source in [".", "/dev/null"] {
        rejection(
            temp.path(),
            &["send", "a", "--body-file", source],
            "invalid_file",
            "--body-file",
            true,
        );
    }
    let socket = temp.path().join("PRIVATE.sock");
    let _owner = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    rejection(
        temp.path(),
        &["send", "a", "--body-file", socket.to_str().unwrap()],
        "invalid_file",
        "--body-file",
        true,
    );
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 2);
}

fn capture_send(shell: &str, stdin: Option<std::process::Stdio>) -> Value {
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::net::UnixListener,
        process::Stdio,
        time::Duration,
    };
    let temp = tempfile::Builder::new()
        .prefix("lx-file-")
        .tempdir_in("/tmp")
        .unwrap();
    std::fs::create_dir(temp.path().join("runtime")).unwrap();
    std::fs::write(temp.path().join("-"), "literal dash\n").unwrap();
    std::os::unix::fs::symlink(temp.path().join("-"), temp.path().join("link")).unwrap();
    let socket = UnixListener::bind(temp.path().join("runtime/control.sock")).unwrap();
    socket.set_nonblocking(true).unwrap();
    let mut command = support::command("/bin/bash");
    command
        .arg("-c")
        .arg(shell)
        .current_dir(temp.path())
        .env("HIVE_TEST_BINARY", env!("CARGO_BIN_EXE_hivectl"))
        .env("HIVE_TEST_STATE", temp.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(stdin) = stdin {
        command.stdin(stdin);
    }
    let mut child = support::ChildGuard::new(command.spawn().unwrap());
    let (mut stream, _) = support::wait::wait_until(
        Duration::from_secs(5),
        Duration::from_millis(5),
        "content request",
        || match socket.accept() {
            Ok(value) => Some(value),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => None,
            Err(error) => panic!("accept: {error}"),
        },
    );
    // BSD sockets inherit the listener's O_NONBLOCK; the timeouts need blocking I/O.
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line).unwrap();
    let request = serde_json::from_str::<Value>(&line).unwrap();
    writeln!(stream, "{{}}").unwrap();
    let status = support::wait::wait_until(
        Duration::from_secs(5),
        Duration::from_millis(5),
        "content command",
        || child.try_wait().unwrap(),
    );
    assert!(status.success());
    assert!(socket.accept().is_err());
    request
}

#[test]
fn pipe_heredoc_process_substitution_literal_dash_and_symlink_keep_exact_bytes() {
    use std::io::Write;
    let invoke = r#""$HIVE_TEST_BINARY" --state-dir "$HIVE_TEST_STATE" send agent --body-file"#;
    let (input, mut writer) = std::io::pipe().unwrap();
    writer.write_all("pipe\n💡\n".as_bytes()).unwrap();
    drop(writer);
    let response = capture_send(&format!("{invoke} /dev/stdin"), Some(input.into()));
    assert_eq!(response["args"]["body"], "pipe\n💡\n");
    for (argument, expected) in [
        ("/dev/stdin <<'HIVE_EOF'\nheredoc\nHIVE_EOF", "heredoc\n"),
        ("<(printf 'substitution\\n')", "substitution\n"),
        ("-", "literal dash\n"),
        ("link", "literal dash\n"),
    ] {
        assert_eq!(
            capture_send(&format!("{invoke} {argument}"), None)["args"]["body"],
            expected
        );
    }
}

#[test]
fn attachment_count_remains_an_input_error_before_startup() {
    use std::io::Write;
    let temp = tempfile::tempdir().unwrap();
    for mut args in [
        vec!["spawn", "--cwd", "/tmp", "--body", "work"],
        vec!["send", "agent", "--body-file", "missing"],
    ] {
        // Send reads its body first; use stdin with exact bounded content.
        if args[0] == "send" {
            args[3] = "/dev/stdin";
        }
        for _ in 0..11 {
            args.extend(["--attach", "PRIVATE"]);
        }
        let (input, mut writer) = std::io::pipe().unwrap();
        writer.write_all(b"work").unwrap();
        drop(writer);
        let output = command(temp.path(), &args).stdin(input).output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["error"]["code"], "out_of_range");
        assert_eq!(value["error"]["field"], "--attach");
        assert!(
            !String::from_utf8(output.stderr)
                .unwrap()
                .contains("PRIVATE")
        );
    }
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[test]
fn inline_text_and_control_ids_reject_before_defaults_or_startup() {
    let temp = tempfile::tempdir().unwrap();
    for operation in ["send", "complete"] {
        let file_flag = if operation == "send" {
            "--body-file"
        } else {
            "--result-file"
        };
        for args in [
            vec![operation, "agent"],
            vec![
                operation,
                "agent",
                "--body",
                "SECRET_CONTENT",
                file_flag,
                "PRIVATE",
            ],
        ] {
            let output = command(temp.path(), &args).output().unwrap();
            assert_eq!(output.status.code(), Some(2));
            let value: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert!(matches!(
                value["error"]["code"].as_str(),
                Some("missing_argument" | "conflicting_arguments")
            ));
            let rendered = format!(
                "{}{}",
                String::from_utf8(output.stdout).unwrap(),
                String::from_utf8(output.stderr).unwrap()
            );
            assert!(!rendered.contains("SECRET_CONTENT"));
            assert!(!rendered.contains("PRIVATE"));
            assert!(!rendered.contains("absent-managed"));
        }
        for (body, code) in [
            (String::new(), "invalid_value"),
            ("💡".repeat(16384) + "x", "out_of_range"),
        ] {
            rejection(
                temp.path(),
                &[operation, "agent", "--body", &body],
                code,
                "--body",
                false,
            );
        }
    }
    for operation in ["spawn", "send", "complete"] {
        let output = command(temp.path(), &[operation, "agent", "--body"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["error"]["code"], "missing_argument");
    }
    for operation in ["stop", "resume", "interrupt"] {
        for key in ["PRIVATE\nID".to_owned(), "x".repeat(129), String::new()] {
            rejection(
                temp.path(),
                &[operation, "agent", "--request-id", &key],
                "invalid_request_id",
                "--request-id",
                false,
            );
        }
    }
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[test]
fn spawn_directory_and_lifetime_errors_precede_content_and_startup() {
    use std::os::unix::ffi::OsStringExt;
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("PRIVATE-file"), "fixture").unwrap();
    for path in ["", "PRIVATE-missing", "PRIVATE-file"] {
        rejection(
            temp.path(),
            &["spawn", "--cwd", path, "--body-file", "missing-body"],
            "invalid_path",
            "--cwd",
            true,
        );
    }
    let invalid = std::ffi::OsString::from_vec(b"PRIVATE\xff".to_vec());
    let output = command(temp.path(), &["spawn", "--body", "work", "--cwd"])
        .arg(invalid)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["code"], "invalid_path");
    assert_eq!(value["error"]["field"], "--cwd");
    assert!(!value.to_string().contains("PRIVATE"));
    let human = String::from_utf8(output.stderr).unwrap();
    assert!(human.contains("PRIVATE\\xFF"));
    rejection(
        temp.path(),
        &[
            "spawn",
            "--cwd",
            "PRIVATE-missing",
            "--body-file",
            "missing-body",
            "--lifetime",
            "with_parent",
        ],
        "invalid_value",
        "--lifetime",
        false,
    );
    let help = support::command(env!("CARGO_BIN_EXE_hivectl"))
        .args(["spawn", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(!help.contains("with_parent"));
    assert!(help.contains("independent"));
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[test]
#[cfg(feature = "comb")]
fn evidence_options_precede_file_access_and_summary_diagnostics_remain_private() {
    let temp = tempfile::tempdir().unwrap();
    for (kind, options, code, flag) in [
        ("review", vec![], "missing_argument", "--verdict"),
        (
            "correction",
            vec!["--verdict", "findings"],
            "conflicting_arguments",
            "--verdict",
        ),
        (
            "correction",
            vec!["--file", "../SECRET_CONTENT"],
            "invalid_value",
            "--file",
        ),
        (
            "correction",
            vec!["--commit", "SECRET_CONTENT"],
            "invalid_value",
            "--commit",
        ),
    ] {
        let mut args = vec!["evidence", kind, "--summary-file", "PRIVATE"];
        args.extend(options);
        rejection(temp.path(), &args, code, flag, false);
    }
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    rejection(
        temp.path(),
        &["evidence", "correction", "--summary"],
        "missing_argument",
        "--summary",
        false,
    );
    rejection(
        temp.path(),
        &["evidence", "correction", "--summary", ""],
        "invalid_value",
        "--summary",
        false,
    );
    std::fs::write(temp.path().join("PRIVATE"), "<private SECRET_CONTENT").unwrap();
    rejection(
        temp.path(),
        &["evidence", "correction", "--summary-file", "PRIVATE"],
        "invalid_value",
        "--summary-file",
        true,
    );
    rejection(
        temp.path(),
        &[
            "evidence",
            "correction",
            "--summary",
            "-----BEGIN SECRET_CONTENT",
        ],
        "invalid_value",
        "--summary",
        false,
    );
}

#[test]
#[cfg(feature = "comb")]
fn evidence_remote_summary_failure_prints_only_the_selected_local_path_to_stderr() {
    use serde_json::json;
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::net::UnixListener,
        time::Duration,
    };
    for file_source in [false, true] {
        let temp = tempfile::tempdir_in("/tmp").unwrap();
        let path = temp.path().join("PRIVATE\nsummary");
        std::fs::write(&path, "Public summary").unwrap();
        std::fs::create_dir(temp.path().join("runtime")).unwrap();
        let listener = UnixListener::bind(hive::runtime::socket_path(temp.path())).unwrap();
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let mut stream = support::wait::wait_until(
                Duration::from_secs(5),
                Duration::from_millis(5),
                "evidence request",
                || match listener.accept() {
                    Ok((stream, _)) => Some(stream),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => None,
                    Err(error) => panic!("{error}"),
                },
            );
            stream.set_nonblocking(false).unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = String::new();
            BufReader::new(&stream).read_line(&mut request).unwrap();
            assert!(!request.contains("PRIVATE"));
            let reply = json!({"request_id":"public-retry", "error":{
                "code":"invalid_value", "message":"Invalid public evidence.", "field":"/summary",
                "expected":"public summary", "examples":[r#"{"op":"evidence","kind":"review","summary":"Public","verdict":"findings"}"#]}});
            writeln!(stream, "{reply}").unwrap();
        });
        let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
            .arg("--state-dir")
            .arg(temp.path())
            .args(["evidence", "correction", "--request-id", "public-retry"])
            .arg(if file_source {
                "--summary-file"
            } else {
                "--summary"
            })
            .arg(if file_source {
                path.as_os_str()
            } else {
                std::ffi::OsStr::new("Public summary")
            })
            .output()
            .unwrap();
        server.join().unwrap();
        assert_eq!(output.status.code(), Some(2));
        let wire: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            wire["error"]["field"],
            if file_source {
                "--summary-file"
            } else {
                "--summary"
            }
        );
        assert_eq!(wire["request_id"], "public-retry");
        assert!(!wire.to_string().contains("PRIVATE"));
        let human = String::from_utf8(output.stderr).unwrap();
        assert_eq!(human.contains("Path:"), file_source);
        assert_eq!(human.contains("PRIVATE\\nsummary"), file_source);
        assert!(!human.contains("PRIVATE\nsummary"));
    }
}

#[test]
fn approval_cursor_errors_precede_defaults_and_never_echo_values() {
    let temp = tempfile::tempdir().unwrap();
    for cursor in [
        "PRIVATE",
        "[1,\"PRIVATE\"]",
        "{}",
        "{\"created_at\":1,\"id\":\"PRIVATE\",\"extra\":true}",
        "{\"created_at\":1,\"id\":\"PRIVATE\",\"id\":\"duplicate\"}",
    ] {
        rejection(
            temp.path(),
            &["approvals", "--after", cursor],
            "invalid_json",
            "--after",
            false,
        );
    }
    for (cursor, code) in [
        (
            r#"{"created_at":9223372036854775808,"id":"PRIVATE"}"#,
            "out_of_range",
        ),
        (r#"{"created_at":0,"id":""}"#, "invalid_value"),
    ] {
        rejection(
            temp.path(),
            &["approvals", "--after", cursor],
            code,
            "--after",
            false,
        );
    }
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[test]
fn batch_headers_and_deadlines_fail_before_defaults_and_startup() {
    let temp = tempfile::tempdir().unwrap();
    for seconds in ["0", "9223372036854775807", "18446744073709551615"] {
        let output = command(
            temp.path(),
            &[
                "batch",
                "--join",
                "--deadline-seconds",
                seconds,
                "--operations-file",
                "PRIVATE-absent",
            ],
        )
        .output()
        .unwrap();
        assert_eq!(output.status.code(), Some(2));
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["error"]["field"], "--deadline-seconds");
        assert!(!String::from_utf8_lossy(&output.stderr).contains("Path:"));
    }
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    let path = temp.path().join("PRIVATE");
    for (value, code) in [
        (serde_json::json!([]), "out_of_range"),
        (
            serde_json::json!(vec![serde_json::json!({}); 65]),
            "out_of_range",
        ),
        (serde_json::json!([null]), "invalid_value"),
        (serde_json::json!([{"op":"send"}]), "missing_argument"),
        (
            serde_json::json!([{"op":"PRIVATE","request_id":"SECRET_CONTENT\n"}]),
            "invalid_request_id",
        ),
        (
            serde_json::json!([{"op":"PRIVATE","request_id":"valid"}]),
            "invalid_value",
        ),
    ] {
        std::fs::write(&path, value.to_string()).unwrap();
        rejection(
            temp.path(),
            &["batch", "--operations-file", "PRIVATE"],
            code,
            "--operations-file",
            true,
        );
    }
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
}
