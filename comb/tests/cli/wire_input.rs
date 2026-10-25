use super::*;
use std::{
    io::BufRead,
    os::unix::{fs::PermissionsExt, net::UnixListener},
};

#[test]
fn transported_input_error_maps_to_cli_field_and_exit_without_losing_receipt() -> TestResult<()> {
    for (field, exit, expected) in [
        ("/operation/request/limit", 2, Some("--limit")),
        ("/operation/request/kind", 2, Some("--type")),
        ("/private-marker", 1, None),
        ("/operation/slug", 1, None),
        ("/operation/request", 2, Some("search")),
    ] {
        check_reply(
            "search",
            &["search", "architecture", "--mode", "text"],
            None,
            field,
            exit,
            expected,
        )?;
    }
    Ok(())
}

#[test]
fn write_reply_fields_match_the_command_and_selected_input_source() -> TestResult<()> {
    let hash = "a".repeat(64);
    for (op, args, field, expected) in [
        (
            "capture",
            vec!["capture", "Evidence"],
            "/operation/text",
            "TEXT",
        ),
        (
            "append",
            vec!["append", "concepts/page", "Evidence", "--source", "review"],
            "/operation/source",
            "--source",
        ),
        (
            "remove",
            vec!["rm", "concepts/page", "--expected-hash", &hash],
            "/operation/expected_hash",
            "--expected-hash",
        ),
        (
            "decide",
            vec![
                "decide",
                "review/page",
                "accept",
                "--reason",
                "Reviewed",
                "--expected-hash",
                &hash,
            ],
            "/operation/decision",
            "DECISION",
        ),
    ] {
        check_reply(op, &args, None, field, 2, Some(expected))?;
    }
    check_reply(
        "capture",
        &["capture", "Evidence"],
        None,
        "/operation/slug",
        1,
        None,
    )?;
    check_reply(
        "put",
        &["put", "concepts/page"],
        Some("---\ntitle: Page\n---\nBody\n"),
        "/operation/markdown",
        2,
        Some("stdin"),
    )?;
    let file = tempfile::NamedTempFile::new()?;
    std::fs::write(file.path(), "---\ntitle: Page\n---\nBody\n")?;
    check_reply(
        "put",
        &[
            "put",
            "concepts/page",
            "--file",
            file.path().to_str().ok_or("path")?,
        ],
        None,
        "/operation/markdown",
        2,
        Some("--file"),
    )?;
    Ok(())
}

#[test]
fn administrative_reply_fields_use_actual_flags_and_reject_foreign_coordinates() -> TestResult<()> {
    let dir = tempfile::tempdir()?;
    let output = dir.path().join("client.json");
    let out = output.to_str().ok_or("output path")?;
    for (op, args, field, exit, expected) in [
        (
            "import",
            vec!["import", "."],
            "/operation/directory",
            2,
            Some("DIR"),
        ),
        (
            "import",
            vec!["import", "."],
            "/operation/prefix",
            2,
            Some("--prefix"),
        ),
        (
            "credential_revoke",
            vec![
                "credential",
                "revoke",
                "12345678-1234-4234-8234-123456789abc",
            ],
            "/operation/id",
            2,
            Some("ID"),
        ),
        (
            "credential_create",
            vec!["credential", "create", "client", "--out", out],
            "/operation/grant/name",
            2,
            Some("NAME"),
        ),
        (
            "credential_create",
            vec!["credential", "create", "client", "--out", out, "--bridge"],
            "/operation/grant/reviewer_roles/1",
            2,
            Some("--reviewer-role"),
        ),
        (
            "credential_create",
            vec!["credential", "create", "client", "--out", out],
            "/operation/grant/reviewer_roles/64",
            1,
            None,
        ),
        (
            "credential_list",
            vec!["credential", "list"],
            "/operation/grant/name",
            1,
            None,
        ),
        ("stats", vec!["stats"], "/operation", 2, Some("stats")),
    ] {
        check_reply(op, &args, None, field, exit, expected)?;
        assert!(!output.exists());
        assert_eq!(
            std::fs::read_dir(dir.path())?.count(),
            0,
            "output reservation leaked"
        );
    }
    Ok(())
}

fn check_reply(
    op: &'static str,
    args: &[&str],
    stdin: Option<&str>,
    field: &'static str,
    exit: i32,
    expected: Option<&str>,
) -> TestResult<()> {
    let dir = tempfile::tempdir_in("/tmp")?;
    drop(comb::Comb::init(dir.path())?);
    let socket = dir.path().join("wire.sock");
    let listener = UnixListener::bind(&socket)?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let server = std::thread::spawn(move || reply(listener, op, field));
    let mut command = Command::new(env!("CARGO_BIN_EXE_comb"));
    command.args(args);
    if comb_protocol::service::input::mutation::Write::named(op).is_some() {
        command.args(["--request-id", "submitted-id"]);
    }
    let mut child = command
        .arg("--root")
        .arg(dir.path())
        .arg("--socket")
        .arg(&socket)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(input) = stdin {
        child
            .stdin
            .take()
            .ok_or("stdin")?
            .write_all(input.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    server.join().map_err(|_| "socket fixture panicked")??;
    assert_eq!(
        output.status.code(),
        Some(exit),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["error"]["field"].as_str(), expected);
    assert_eq!(response["error"]["file_committed"], true);
    // Mutating plans retain their submitted retry identity; reads retain the server's.
    assert!(response["error"]["request_id"].is_string());
    assert_eq!(
        response["error"]["request_id"],
        if comb_protocol::service::input::mutation::Write::named(op).is_some() {
            "submitted-id"
        } else {
            "retained-id"
        }
    );
    let stderr = String::from_utf8(output.stderr)?;
    if let Some(expected) = expected {
        assert!(
            response["error"]["examples"][0]
                .as_str()
                .is_some_and(|text| text.contains("comb"))
        );
        assert!(stderr.contains(expected));
        assert!(stderr.contains("Example"));
    } else {
        assert_eq!(response["error"]["code"], "invalid_frame");
        assert!(!response.to_string().contains(field));
        assert!(!stderr.contains(field));
    }
    Ok(())
}

fn reply(listener: UnixListener, op: &str, field: &str) -> std::io::Result<()> {
    let start = std::time::Instant::now();
    let stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    && start.elapsed().as_secs() < 10 =>
            {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(error) => return Err(error),
        }
    };
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(5)))?;
    let mut stream = std::io::BufReader::new(stream);
    let mut request = String::new();
    stream.read_line(&mut request)?;
    let request: Value = serde_json::from_str(&request)?;
    assert_eq!(request["operation"]["op"], op);
    let reply = serde_json::json!({"protocol":1,"status":"error","error":{
        "code":"out_of_range","message":"Invalid retrieval input.",
        "field":field, "expected":"an integer from 1 through 100",
        "examples":["operation: search"], "file_committed":true,"request_id":"retained-id"
    }});
    writeln!(stream.get_mut(), "{reply}")?;
    Ok(())
}
