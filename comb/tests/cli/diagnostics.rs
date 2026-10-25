use super::*;

#[test]
fn malformed_frontmatter_is_a_private_input_error_before_execution() {
    let dir = tempfile::tempdir().unwrap();
    cli(dir.path(), &["init"], None, true).unwrap();
    let marker = "private-frontmatter-marker";
    let markdown = format!("---\ntitle: Diagnostics\nexternal: {marker}\n---\nText\n");
    let error = comb::Page::parse(&markdown).unwrap_err();
    assert!(
        std::error::Error::source(&error)
            .unwrap()
            .to_string()
            .contains(marker)
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_comb"))
        .arg("--root")
        .arg(dir.path())
        .args([
            "put",
            "concepts/diagnostics",
            "--request-id",
            "frontmatter-rejection",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(markdown.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let reply: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(reply["error"]["code"], "invalid_value");
    assert_eq!(reply["error"]["field"], "stdin");
    assert!(reply["error"].get("request_id").is_none());
    assert!(
        reply["error"]["expected"]
            .as_str()
            .unwrap()
            .contains("Markdown")
    );
    assert!(!stdout.contains(marker));
    assert!(!String::from_utf8(output.stderr).unwrap().contains(marker));
}

#[test]
fn broken_input_diagnostic_stderr_preserves_json_and_reports_output_failure() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("comb");
    cli(&root, &["init"], None, true).unwrap();
    let marker = "private-frontmatter-marker";
    let markdown = format!("---\ntitle: Diagnostics\nexternal: {marker}\n---\nText\n");
    assert!(
        comb::Page::parse(&markdown)
            .unwrap_err()
            .diagnostic()
            .unwrap()
            .contains(marker)
    );
    let page = dir.path().join("malformed.md");
    std::fs::write(&page, markdown).unwrap();
    let (writer, reader) = std::os::unix::net::UnixStream::pair().unwrap();
    drop(reader);
    let output = Command::new(env!("CARGO_BIN_EXE_comb"))
        .arg("--root")
        .arg(root)
        .args([
            "put",
            "concepts/diagnostics",
            "--request-id",
            "frontmatter-rejection",
            "--file",
        ])
        .arg(page)
        .stdin(Stdio::null())
        .stderr(Stdio::from(std::os::fd::OwnedFd::from(writer)))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let reply: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(reply["error"]["code"], "invalid_value");
    assert_eq!(reply["error"]["field"], "--file");
    assert!(reply["error"].get("request_id").is_none());
    assert!(reply["error"]["examples"].is_array());
    assert!(!stdout.contains(marker));
}

#[test]
fn invalid_serve_socket_identifies_input_and_survives_unwritable_stderr() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    cli(dir.path(), &["init"], None, true).unwrap();
    let socket = dir.path().join("missing/comb.sock");
    for writable in [true, false] {
        let stderr = if writable {
            Stdio::piped()
        } else {
            {
                let (writer, reader) = std::os::unix::net::UnixStream::pair().unwrap();
                drop(reader);
                Stdio::from(std::os::fd::OwnedFd::from(writer))
            }
        };
        let output = Command::new(env!("CARGO_BIN_EXE_comb"))
            .arg("--root")
            .arg(dir.path())
            .arg("--socket")
            .arg(&socket)
            .arg("serve")
            .stdin(Stdio::null())
            .stderr(stderr)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(if writable { 2 } else { 1 }));
        let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(reply["error"]["code"], "invalid_path");
        assert_eq!(reply["error"]["field"], "--socket");
        assert!(reply["error"]["examples"].is_array());
        if writable {
            let stderr = String::from_utf8(output.stderr).unwrap();
            assert!(stderr.contains("--socket"));
            assert!(stderr.contains("Path:"));
        }
    }
}

#[test]
fn fatal_mcp_credential_errors_explain_the_field_without_private_parser_content() {
    let dir = tempfile::tempdir().unwrap();
    let marker = "private-credential-marker";
    let credential = serde_json::json!({
        "format": 1,
        "socket": "/tmp/unused-comb.sock",
        "credential": marker,
    })
    .to_string();
    assert!(
        serde_json::from_str::<comb::service::auth::Connection>(&credential)
            .err()
            .unwrap()
            .to_string()
            .contains(marker)
    );
    // NamedTempFile creates an owned 0600 file, so parsing is the failure boundary.
    let mut file = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
    file.write_all(credential.as_bytes()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_comb"))
        .args(["mcp", "--credential"])
        .arg(file.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("--credential"));
    assert!(diagnostic.contains("invalid_file"));
    assert!(diagnostic.contains("comb mcp --credential"));
    assert!(!diagnostic.contains(marker));
}
