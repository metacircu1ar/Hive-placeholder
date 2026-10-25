//! Caller payload failures precede all knowledge or service access.
use std::{
    collections::BTreeMap,
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn command(root: &Path, socket: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_comb"));
    command
        .env_remove("HOME")
        .env("XDG_DATA_HOME", "invalid-relative")
        .arg("--root")
        .arg(root)
        .arg("--socket")
        .arg(socket)
        .stdin(Stdio::null());
    command
}
fn rejected(output: &Output, field: &str, code: &str) -> Result {
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(value["error"]["field"], field);
    assert_eq!(value["error"]["code"], code);
    assert!(value["error"]["examples"].is_array());
    assert!(value["error"]["expected"].is_string());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("PRIVATE"));
    for bytes in [&output.stdout, &output.stderr] {
        assert!(!String::from_utf8_lossy(bytes).contains("PAYLOAD_SENTINEL"));
        assert!(!bytes.contains(&0x1b));
    }
    Ok(())
}

#[test]
fn files_markdown_and_import_fail_before_missing_root_or_sentinel_connection() -> Result {
    let directory = tempfile::tempdir_in("/tmp")?;
    let root = directory.path().join("absent");
    let socket = directory.path().join("sentinel.sock");
    let listener = UnixListener::bind(&socket)?;
    listener.set_nonblocking(true)?;
    let input = directory.path().join("PRIVATE-input.md");
    let cases = [
        (b"PAYLOAD_SENTINEL".to_vec(), "invalid_value"),
        (
            b"---\ntitle: Valid\nexternal: PAYLOAD_SENTINEL\n---\nText".to_vec(),
            "invalid_value",
        ),
        (vec![0xff], "invalid_file"),
        (vec![b'x'; comb::page::PAGE_BYTES + 1], "out_of_range"),
    ];
    for (bytes, code) in cases {
        std::fs::write(&input, bytes)?;
        let output = command(&root, &socket)
            .args(["put", "page", "--file"])
            .arg(&input)
            .output()?;
        rejected(&output, "--file", code)?;
    }
    let hash = "a".repeat(64);
    for (summary, nonblank) in [(" \n", true), ("PAYLOAD_SENTINEL\0", false)] {
        std::fs::write(&input, summary)?;
        let mut cmd = command(&root, &socket);
        if nonblank {
            cmd.args([
                "--run",
                "run",
                "consolidate",
                "inbox/note",
                "page",
                "--note-hash",
                &hash,
                "--target-hash",
                &hash,
            ]);
        } else {
            cmd.args([
                "append",
                "page",
                "valid",
                "--source",
                "source",
                "--expected-hash",
                &hash,
            ]);
        }
        rejected(
            &cmd.arg("--summary-file").arg(&input).output()?,
            "--summary-file",
            "invalid_value",
        )?;
    }
    rejected(
        &command(&root, &socket).arg("import").arg(&input).output()?,
        "DIR",
        "invalid_path",
    )?;
    let missing = directory.path().join("PRIVATE-missing\n\x1b[2J");
    let output = command(&root, &socket)
        .args(["put", "page", "--file"])
        .arg(&missing)
        .output()?;
    rejected(&output, "--file", "invalid_file")?;
    assert!(String::from_utf8_lossy(&output.stderr).contains("PRIVATE-missing\\n\\x1b"));
    assert!(!root.exists());
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    Ok(())
}

fn snapshot(root: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
    fn visit(root: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) -> Result {
        for entry in std::fs::read_dir(root)? {
            let path = entry?.path();
            if path.is_dir() {
                visit(&path, result)?;
            } else {
                result.insert(path.clone(), std::fs::read(path)?);
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    visit(root, &mut files)?;
    Ok(files)
}

#[test]
fn unreadable_import_directory_fails_before_knowledge_or_socket_access() -> Result {
    use std::os::unix::fs::PermissionsExt;

    // Root can enumerate mode-0111 directories; its result cannot test this rule.
    if rustix::process::geteuid().is_root() {
        return Ok(());
    }
    let directory = tempfile::tempdir_in("/tmp")?;
    let root = directory.path().join("absent");
    let input = directory.path().join("PRIVATE-unreadable");
    std::fs::create_dir(&input)?;
    let socket = directory.path().join("sentinel.sock");
    let listener = UnixListener::bind(&socket)?;
    listener.set_nonblocking(true)?;
    std::fs::set_permissions(&input, std::fs::Permissions::from_mode(0o111))?;
    let access = std::fs::read_dir(&input);
    let output = command(&root, &socket).arg("import").arg(&input).output();
    std::fs::set_permissions(&input, std::fs::Permissions::from_mode(0o700))?;
    assert_eq!(
        access.unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    rejected(&output?, "DIR", "invalid_path")?;
    assert!(!root.exists());
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    Ok(())
}

#[test]
fn rejected_payload_does_not_reconcile_an_external_edit_or_touch_the_index() -> Result {
    let directory = tempfile::tempdir_in("/tmp")?;
    let root = directory.path().join("knowledge");
    drop(comb::Comb::init(&root)?);
    let input = directory.path().join("input.md");
    std::fs::write(&input, "---\ntitle: Page\n---\nOriginal")?;
    let socket = directory.path().join("unused.sock");
    let output = command(&root, &socket)
        .args(["put", "page", "--file"])
        .arg(&input)
        .output()?;
    assert!(output.status.success());
    std::fs::write(root.join("page.md"), "external malformed edit")?;
    std::fs::write(&input, "PAYLOAD_SENTINEL")?;
    let before = snapshot(&root)?;
    let output = command(&root, &socket)
        .args(["put", "page", "--file"])
        .arg(&input)
        .output()?;
    rejected(&output, "--file", "invalid_value")?;
    assert_eq!(snapshot(&root)?, before);
    Ok(())
}
