//! Parser and display paths must finish before knowledge or service work.
use std::{
    os::unix::net::UnixListener,
    process::{Command, Stdio},
};

#[test]
fn parsing_and_help_never_create_roots_or_contact_the_socket() {
    let directory = tempfile::tempdir_in("/tmp").unwrap();
    let root = directory.path().join("absent-root");
    let socket = directory.path().join("sentinel.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    for (args, code) in [
        (vec!["--help"], 0),
        (vec!["--version"], 0),
        (vec!["model", "use-http", "--help"], 0),
        (vec!["setup", "--no-services", "--services"], 2),
        (vec!["search", "PRIVATE_QUERY", "--limit", "bad"], 2),
        (vec!["mcp"], 2),
        (vec!["credential", "create", "PRIVATE_NAME"], 2),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_comb"))
            .current_dir(directory.path())
            .env_remove("HOME")
            .env("XDG_DATA_HOME", "invalid-relative")
            .arg("--root")
            .arg(&root)
            .arg("--socket")
            .arg(&socket)
            .args(&args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(code),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        if code == 2 {
            let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert!(value["error"]["examples"].is_array());
            assert!(!output.stderr.is_empty());
        } else {
            assert!(output.stderr.is_empty());
        }
        for bytes in [&output.stdout, &output.stderr] {
            assert!(!String::from_utf8_lossy(bytes).contains("PRIVATE_"));
        }
        assert!(!root.exists());
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}

#[test]
fn pure_input_rejection_precedes_stdin_files_and_service_work() {
    let directory = tempfile::tempdir_in("/tmp").unwrap();
    let root = directory.path().join("absent-root");
    let socket = directory.path().join("sentinel.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let hash = "a".repeat(64);
    for args in input_cases(&hash).into_iter().chain(runtime_cases()) {
        assert_rejected(&directory, &root, &socket, &listener, &args).unwrap();
    }
    for base in [
        vec!["setup", "--for-hive", "--no-services"],
        vec![
            "credential",
            "create",
            "label",
            "--out",
            "PRIVATE.json",
            "--bridge",
        ],
    ] {
        let roles: Vec<_> = (0..65).map(|i| format!("role{i}")).collect();
        let mut args = base;
        for role in &roles {
            args.extend(["--reviewer-role", role]);
        }
        assert_rejected(&directory, &root, &socket, &listener, &args).unwrap();
    }
}

fn input_cases(hash: &str) -> Vec<Vec<&str>> {
    vec![
        vec!["put", "PRIVATE-invalid-slug"],
        vec![
            "put",
            "page",
            "--expected-hash",
            "PRIVATE",
            "--file",
            "PRIVATE-missing.md",
        ],
        vec![
            "append",
            "page",
            "PRIVATE",
            "--source",
            "PRIVATE",
            "--summary-file",
            "PRIVATE-missing.md",
        ],
        vec![
            "consolidate",
            "inbox/note",
            "page",
            "--note-hash",
            hash,
            "--target-hash",
            hash,
            "--summary-file",
            "PRIVATE-missing.md",
        ],
        vec!["search", "PRIVATE", "--limit", "101"],
        vec![
            "search",
            "PRIVATE",
            "--mode",
            "text",
            "--embedding-config",
            "PRIVATE-missing.yaml",
        ],
    ]
}

fn runtime_cases() -> Vec<Vec<&'static str>> {
    vec![
        vec!["list", "--after", "PRIVATE"],
        vec!["graph", "page", "--depth", "9"],
        vec!["serve", "--vector-cache-mib", "0"],
        vec![
            "credential",
            "create",
            "PRIVATE\n",
            "--out",
            "PRIVATE-output.json",
        ],
        vec![
            "credential",
            "create",
            "PRIVATE",
            "--out",
            "PRIVATE-output.json",
            "--bridge",
            "--reviewer-role",
            "PRIVATE",
            "--reviewer-role",
            "PRIVATE",
        ],
        vec![
            "setup",
            "--for-hive",
            "--no-services",
            "--reviewer-role",
            "PRIVATE",
            "--reviewer-role",
            "PRIVATE",
        ],
        vec![
            "--run",
            "PRIVATE\n",
            "mcp",
            "--credential",
            "PRIVATE-missing.json",
        ],
    ]
}

#[test]
fn unused_globals_are_rejected_before_payloads_credentials_and_home_lookup() {
    let directory = tempfile::tempdir_in("/tmp").unwrap();
    let root = directory.path().join("absent-root");
    let socket = directory.path().join("sentinel.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    for args in [
        vec!["init"],
        vec!["version"],
        vec!["mcp", "--credential", "PRIVATE-missing.json"],
        vec!["embed", "--config", "PRIVATE-missing.yaml"],
        vec!["sync", "--rebuild"],
        vec!["model", "use", "PRIVATE"],
        vec!["service", "status"],
        vec!["setup-maintenance"],
        vec![
            "put",
            "page",
            "--managed-service",
            "PRIVATE",
            "--file",
            "PRIVATE.md",
        ],
        vec!["credential", "list", "--run", "PRIVATE"],
        vec!["setup", "--managed-service", "PRIVATE.json"],
    ] {
        assert_rejected(&directory, &root, &socket, &listener, &args).unwrap();
    }
}

#[test]
fn version_modes_need_no_state_and_detect_broken_output() {
    let directory = tempfile::tempdir_in("/tmp").unwrap();
    let info = comb::build_info();
    let command = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_comb"));
        command
            .current_dir(directory.path())
            .env_remove("HOME")
            .env("XDG_DATA_HOME", "invalid-relative")
            .stdin(Stdio::null());
        command
    };
    let human = command().arg("version").output().unwrap();
    assert_eq!(human.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(human.stdout).unwrap(),
        format!(
            "Comb {} ({})\n",
            info.product_version,
            info.compiled_features.join(",")
        )
    );
    assert!(human.stderr.is_empty());
    let json = command().args(["version", "--json"]).output().unwrap();
    assert_eq!(json.status.code(), Some(0));
    assert!(json.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(value, serde_json::to_value(info).unwrap());
    for args in [
        vec!["version"],
        vec!["version", "--json"],
        vec!["--version"],
    ] {
        let (writer, reader) = std::os::unix::net::UnixStream::pair().unwrap();
        drop(reader);
        let output = command()
            .args(&args)
            .stdout(Stdio::from(std::os::fd::OwnedFd::from(writer)))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stderr.is_empty());
    }
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

fn assert_rejected(
    directory: &tempfile::TempDir,
    root: &std::path::Path,
    socket: &std::path::Path,
    listener: &UnixListener,
    args: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_comb"))
        .current_dir(directory.path())
        .env_remove("HOME")
        .env("XDG_DATA_HOME", "invalid-relative")
        .arg("--root")
        .arg(root)
        .arg("--socket")
        .arg(socket)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // Keep the writer alive: an accidental stdin read cannot finish on EOF.
    let stdin = child.stdin.take().ok_or("stdin is not piped")?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while child.try_wait()?.is_none() {
        if std::time::Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            panic!("input rejection blocked: {args:?}");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let output = child.wait_with_output()?;
    drop(stdin);
    assert_eq!(
        output.status.code(),
        Some(2),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reply: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert!(reply["error"]["expected"].is_string());
    assert!(reply["error"]["examples"].is_array());
    for bytes in [&output.stdout, &output.stderr] {
        assert!(!String::from_utf8_lossy(bytes).contains("PRIVATE"));
    }
    assert!(!root.exists());
    assert_eq!(
        match listener.accept() {
            Err(cause) => cause.kind(),
            Ok(_) => panic!("unexpected socket connection"),
        },
        std::io::ErrorKind::WouldBlock
    );
    assert_eq!(std::fs::read_dir(directory.path())?.count(), 1);
    Ok(())
}
