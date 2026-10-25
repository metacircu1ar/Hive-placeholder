//! Configuration failures must not reach knowledge or a serving process.
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::net::UnixListener,
    path::Path,
    process::{Command, Output},
};

fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_comb"));
    command.arg("--root").arg(root);
    command
}
fn rejected(output: Output, field: &str, code: &str) -> super::TestResult<()> {
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(value["error"]["field"], field);
    assert_eq!(value["error"]["code"], code);
    assert!(value["error"]["examples"].is_array());
    for bytes in [&output.stdout, &output.stderr] {
        assert!(!String::from_utf8_lossy(bytes).contains("PAYLOAD_SENTINEL"));
        assert!(!bytes.contains(&0x1b));
    }
    assert!(!String::from_utf8_lossy(&output.stdout).contains("PRIVATE"));
    Ok(())
}
#[test]
fn bad_configs_precede_root_access_and_sentinel_connection() -> super::TestResult<()> {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let root = dir.path().join("absent");
    let socket = dir.path().join("sentinel.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let path = dir.path().join("PRIVATE-\n\x1b.json");
    let cases = [
        (b"PAYLOAD_SENTINEL: [".to_vec(), "invalid_file"),
        (vec![0xff], "invalid_file"),
        (
            vec![b'x'; comb::embedding::CONFIG_BYTES + 1],
            "out_of_range",
        ),
    ];
    for (body, code) in cases {
        fs::write(&path, body).unwrap();
        rejected(
            command(&root)
                .args(["embed", "--config"])
                .arg(&path)
                .output()
                .unwrap(),
            "--config",
            code,
        )?;
        for args in [
            vec!["serve", "--embedding-config"],
            vec!["search", "query", "--embedding-config"],
        ] {
            rejected(
                command(&root)
                    .arg("--socket")
                    .arg(&socket)
                    .args(args)
                    .arg(&path)
                    .output()
                    .unwrap(),
                "--embedding-config",
                code,
            )?;
        }
    }
    fs::remove_file(&path).unwrap();
    rejected(
        command(&root)
            .args(["embed", "--config"])
            .arg(&path)
            .output()
            .unwrap(),
        "--config",
        "invalid_file",
    )?;
    assert!(!root.exists());
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    Ok(())
}
#[test]
fn root_preflight_precedes_engine_load_but_explicit_engine_failures_never_fallback() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let root = dir.path().join("absent");
    let path = dir.path().join("bad-model.json");
    fs::write(
        &path,
        json!({"backend":"http","model":"tiny","revision":"pin","dimensions":3,"url":"http://127.0.0.1:9/v1/embeddings","api_key_env":"COMB_CONFIG_MISSING_KEY"})
            .to_string(),
    )
    .unwrap();
    for initialized in [false, true] {
        if initialized {
            drop(comb::Comb::init(&root).unwrap());
        }
        let index = root.join(".comb/index.sqlite");
        let before = super::support::optional_bytes(&index);
        for args in [
            vec!["search", "query", "--embedding-config"],
            vec!["embed", "--config"],
            vec!["serve", "--embedding-config"],
        ] {
            let output = command(&root)
                .env_remove("COMB_CONFIG_MISSING_KEY")
                .args(args)
                .arg(&path)
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(if initialized { 1 } else { 2 }));
            let value: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                value["error"]["code"],
                if initialized {
                    "embedding_credentials"
                } else {
                    "invalid_path"
                }
            );
            if !initialized {
                assert_eq!(value["error"]["field"], "--root");
            }
            assert!(value.get("fallback").is_none());
            assert_eq!(root.exists(), initialized);
            assert_eq!(super::support::optional_bytes(&index), before);
        }
    }
}

#[test]
fn model_free_serve_options_fail_before_index_sync_or_socket_creation() -> super::TestResult<()> {
    let dir = tempfile::tempdir_in("/tmp")?;
    let root = dir.path().join("root");
    drop(comb::Comb::init(&root)?);
    let index = root.join(".comb/index.sqlite");
    let before = fs::read(&index)?;
    let external = root.join("external.md");
    fs::write(&external, "PRIVATE unsynced page")?;
    for args in [
        vec!["--replace-embeddings"],
        vec!["--retry-failed"],
        vec!["--vector-cache-mib", "512"],
    ] {
        rejected(
            command(&root).arg("serve").args(&args).output()?,
            args[0],
            "missing_argument",
        )?;
        assert_eq!(fs::read(&index)?, before);
        assert_eq!(fs::read_to_string(&external)?, "PRIVATE unsynced page");
        assert!(!root.join(".comb/service.sock").exists());
        assert!(!fs::read_dir(root.join(".comb"))?.any(|p| {
            p.unwrap()
                .file_name()
                .to_string_lossy()
                .contains("comb-socket")
        }));
    }
    Ok(())
}
