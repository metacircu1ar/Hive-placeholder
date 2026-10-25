#![allow(
    clippy::disallowed_methods,
    reason = "Raw writes are gated by the exact the reviewed maintainer write inventory inventory"
)]
//! CLI bytes and exit codes. Only explicitly named volatile values are replaced.
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::{
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct Cli {
    root: tempfile::TempDir,
}

fn check_stderr(name: &str, stderr: &[u8]) {
    match name {
        "vector-unavailable" => {
            let text = String::from_utf8_lossy(stderr);
            assert!(text.contains("embeddings_unavailable"));
            assert!(text.contains("--embedding-config FILE"));
            assert!(text.contains("--mode text"));
            assert!(text.contains("Examples:"));
        }
        "embed-error" | "serve-error" => {
            let text = String::from_utf8_lossy(stderr);
            let flag = if name == "embed-error" {
                "--config"
            } else {
                "--embedding-config"
            };
            assert!(text.contains(flag));
            assert!(text.contains("configuration field url"));
            assert!(text.contains("Expected:"));
            assert!(text.contains("comb "));
            assert!(!text.contains("bad-url"));
        }
        "bad-slug" | "bad-search" => {
            let text = String::from_utf8_lossy(stderr);
            assert!(text.contains("Expected:"));
            assert!(text.contains("comb "));
            assert!(!text.contains("../outside"));
        }
        _ => assert!(stderr.is_empty(), "{}", String::from_utf8_lossy(stderr)),
    }
}

fn golden(name: &str, bytes: impl AsRef<[u8]>) -> TestResult {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/contracts")
        .join(format!("cli-{name}"));
    if std::env::var_os("COMB_RECORD_GOLDENS").is_some() {
        let mut file =
            tempfile::NamedTempFile::new_in(path.parent().ok_or("fixture path has no parent")?)?;
        file.write_all(bytes.as_ref())?;
        file.persist(&path)?;
    }
    let expected = std::fs::read(path)?;
    assert!(
        expected == bytes.as_ref(),
        "{name}\nexpected: {}\nactual: {}",
        String::from_utf8_lossy(&expected),
        String::from_utf8_lossy(bytes.as_ref())
    );
    Ok(())
}

impl Cli {
    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_comb"));
        if args.first() != Some(&"mcp") {
            cmd.arg("--root").arg(self.root.path());
        }
        // Protocol byte contracts pin the explicit quiet diagnostic setting;
        // default structured logging is exercised by cli/logging and service tests.
        cmd.args(args)
            .env("COMB_LOG", "off")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }
    fn output(&self, args: &[&str], input: Option<&str>) -> TestResult<Output> {
        let mut child = self.command(args).spawn()?;
        if let Some(input) = input {
            child
                .stdin
                .take()
                .ok_or("child stdin is not piped")?
                .write_all(input.as_bytes())?;
        }
        Ok(child.wait_with_output()?)
    }
    fn record(
        &self,
        name: &str,
        args: &[&str],
        input: Option<&str>,
        volatiles: &[(&str, &str)],
    ) -> TestResult<Value> {
        let output = self.output(args, input)?;
        check_stderr(name, &output.stderr);
        let value: Value = serde_json::from_slice(&output.stdout)?;
        let mut stdout = String::from_utf8(output.stdout)?;
        stdout = self.normalize(name, &value, stdout, volatiles)?;
        golden(
            &format!("{name}.txt"),
            format!(
                "exit={}\n{stdout}",
                output.status.code().ok_or("child terminated by signal")?
            ),
        )?;
        Ok(value)
    }
    fn normalize(
        &self,
        name: &str,
        value: &Value,
        mut stdout: String,
        volatiles: &[(&str, &str)],
    ) -> TestResult<String> {
        for (pointer, kind) in volatiles {
            let raw = value
                .pointer(pointer)
                .unwrap_or_else(|| panic!("{name}: missing {pointer}: {value}"))
                .as_str()
                .ok_or("volatile value is not a string")?;
            match *kind {
                "uuid" => {
                    uuid::Uuid::parse_str(raw)?;
                }
                "hash" => assert!(
                    raw.len() == 64
                        && raw
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                ),
                "path" => assert!(Path::new(raw).starts_with(self.root.path())),
                _ => panic!("unknown volatile kind"),
            }
            stdout = stdout.replace(&serde_json::to_string(raw)?, &format!("\"<{kind}>\""));
        }
        Ok(stdout)
    }
    fn hash(&self, slug: &str) -> TestResult<String> {
        let value: Value = serde_json::from_slice(&self.output(&["get", slug], None)?.stdout)?;
        Ok(value["hash"]
            .as_str()
            .ok_or("document hash is missing")?
            .into())
    }
}

fn setup() -> TestResult<Cli> {
    let cli = Cli {
        root: tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")?,
    };
    cli.record("init", &["init"], None, &[("/comb_id", "uuid")])?;
    // Establish the fixed test identity before any page or credential exists.
    // Sync authenticates external pages, so its seals otherwise vary per run.
    std::fs::write(
        cli.root.path().join(".comb/identity.json"),
        include_bytes!("fixtures/contracts/identity.json"),
    )?;
    for (slug, title, body) in [
        (
            "concepts/page",
            "Writer",
            "A writer retains messages. [[concepts/target]]",
        ),
        ("concepts/target", "Target", "Original summary."),
        ("inbox/note", "Note", "An observation."),
        ("review/proposal", "Proposal", "A proposed improvement."),
    ] {
        let path = cli.root.path().join(format!("{slug}.md"));
        std::fs::create_dir_all(path.parent().ok_or("fixture path has no parent")?)?;
        let page = comb::Page::parse(&format!(
            "---\ntitle: {title}\ntype: concept\ncreated_at: '2026-01-02T03:04:05Z'\nupdated_at: '2026-01-02T03:04:05Z'\n---\n{body}"
        ))?;
        std::fs::write(path, page.render()?)?;
    }
    cli.record("sync", &["sync"], None, &[])?;
    Ok(cli)
}

#[test]
fn golden_cli_read_commands_and_errors() -> TestResult {
    let cli = setup()?;
    for (name, args) in [
        ("get", vec!["get", "concepts/page"]),
        ("list", vec!["list"]),
        ("search", vec!["search", "writer", "--mode", "text"]),
        ("links", vec!["links", "concepts/page"]),
        ("backlinks", vec!["backlinks", "concepts/target"]),
        ("graph", vec!["graph", "concepts/page"]),
        ("stats", vec!["stats"]),
        ("rebuild", vec!["sync", "--rebuild"]),
        ("missing", vec!["get", "concepts/missing"]),
        ("bad-slug", vec!["get", "../outside"]),
        ("bad-search", vec!["search", "", "--limit", "0"]),
        (
            "vector-unavailable",
            vec!["search", "writer", "--mode", "vector"],
        ),
    ] {
        cli.record(name, &args, None, &[])?;
    }
    Ok(())
}

#[test]
fn golden_cli_mutation_commands() -> TestResult {
    let cli = setup()?;
    let mutation = &[("/hash", "hash")];
    record_capture_and_put(&cli, mutation)?;
    record_append_and_capture(&cli, mutation)?;
    record_reviews(&cli, mutation)?;
    record_consolidation(&cli)?;
    record_removal_and_import(&cli)?;
    Ok(())
}

#[test]
fn golden_cli_credentials_and_mcp() -> TestResult {
    let cli = setup()?;
    let credential = cli.root.path().join("client.json");
    let result = cli.record(
        "credential-create",
        &[
            "credential",
            "create",
            "Writer",
            "--out",
            credential.to_str().ok_or("credential path is not UTF-8")?,
        ],
        None,
        &[("/credential_id", "uuid"), ("/file", "path")],
    )?;
    cli.record(
        "credential-list",
        &["credential", "list"],
        None,
        &[("/0/id", "uuid")],
    )?;
    let output = cli.output(
        &[
            "mcp",
            "--credential",
            credential.to_str().ok_or("credential path is not UTF-8")?,
        ],
        Some("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\",\"clientInfo\":{},\"capabilities\":{}}}\n"),
    )?;
    assert!(output.stderr.is_empty());
    golden(
        "mcp.txt",
        format!(
            "exit={}\n{}",
            output.status.code().ok_or("child terminated by signal")?,
            String::from_utf8(output.stdout)?
        ),
    )?;
    cli.record(
        "credential-revoke",
        &[
            "credential",
            "revoke",
            result["credential_id"]
                .as_str()
                .ok_or("credential ID is missing")?,
        ],
        None,
        &[("/revoked", "uuid")],
    )?;
    Ok(())
}

#[test]
fn golden_cli_embedding_and_serve() -> TestResult {
    let cli = Cli {
        root: tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")?,
    };
    cli.record("init", &["init"], None, &[("/comb_id", "uuid")])?;
    record_embedding(&cli)?;
    record_serve_interrupt(&cli)?;
    Ok(())
}

#[test]
fn golden_cli_doctor_does_not_pin_filesystem_traversal_order() -> TestResult {
    let cli = Cli {
        root: tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")?,
    };
    cli.record("init", &["init"], None, &[("/comb_id", "uuid")])?;
    assert!(
        cli.output(
            &["put", "concepts/isolated", "--request-id", "doctor"],
            Some("---\ntitle: Isolated\n---\nAn uncited paragraph.")
        )?
        .status
        .success()
    );
    cli.record("doctor", &["doctor"], None, &[])?;
    Ok(())
}

fn record_capture_and_put(cli: &Cli, mutation: &[(&str, &str)]) -> TestResult {
    cli.record(
        "capture-blank-title",
        &["capture", "\nObservation", "--request-id", "blank"],
        None,
        mutation,
    )?;
    cli.record(
        "put",
        &["put", "concepts/new", "--request-id", "put"],
        Some("---\ntitle: New\n---\nContent"),
        mutation,
    )?;
    cli.record(
        "put-replay",
        &["put", "concepts/new", "--request-id", "put"],
        Some("---\ntitle: New\n---\nContent"),
        mutation,
    )?;
    cli.record(
        "put-conflict",
        &["put", "concepts/new", "--request-id", "conflict"],
        Some("---\ntitle: Changed\n---\nContent"),
        &[],
    )?;
    Ok(())
}

fn record_append_and_capture(cli: &Cli, mutation: &[(&str, &str)]) -> TestResult {
    cli.record(
        "append",
        &[
            "append",
            "concepts/new",
            "Evidence",
            "--source",
            "test",
            "--request-id",
            "append",
        ],
        None,
        mutation,
    )?;
    cli.record(
        "capture",
        &["capture", "Captured note", "--request-id", "capture"],
        None,
        mutation,
    )?;
    Ok(())
}

fn record_reviews(cli: &Cli, mutation: &[(&str, &str)]) -> TestResult {
    cli.record(
        "promote",
        &[
            "promote",
            "concepts/page",
            "--expected-hash",
            &cli.hash("concepts/page")?,
            "--request-id",
            "promote",
        ],
        None,
        mutation,
    )?;
    cli.record(
        "decide",
        &[
            "decide",
            "review/proposal",
            "reject",
            "--reason",
            "Needs evidence",
            "--expected-hash",
            &cli.hash("review/proposal")?,
            "--request-id",
            "decide",
        ],
        None,
        mutation,
    )?;
    Ok(())
}

fn record_consolidation(cli: &Cli) -> TestResult {
    let summary = cli.root.path().join("summary.txt");
    std::fs::write(&summary, "New summary. [evidence:new]")?;
    cli.record(
        "consolidate",
        &[
            "--run",
            "curate",
            "consolidate",
            "inbox/note",
            "concepts/target",
            "--note-hash",
            &cli.hash("inbox/note")?,
            "--target-hash",
            &cli.hash("concepts/target")?,
            "--summary-file",
            summary.to_str().ok_or("summary path is not UTF-8")?,
            "--recopy",
            "--request-id",
            "consolidate",
        ],
        None,
        &[("/target/hash", "hash"), ("/note/hash", "hash")],
    )?;
    Ok(())
}

fn record_removal_and_import(cli: &Cli) -> TestResult {
    cli.record(
        "rm",
        &[
            "rm",
            "concepts/new",
            "--expected-hash",
            &cli.hash("concepts/new")?,
            "--request-id",
            "remove",
        ],
        None,
        &[],
    )?;
    let imported = cli.root.path().join("import");
    std::fs::create_dir(&imported)?;
    std::fs::write(imported.join("note.md"), "Imported text.")?;
    cli.record(
        "import",
        &[
            "import",
            imported.to_str().ok_or("import path is not UTF-8")?,
            "--prefix",
            "concepts",
        ],
        None,
        &[("/batch_id", "uuid")],
    )?;
    Ok(())
}

fn record_embedding(cli: &Cli) -> TestResult {
    let config = cli.root.path().join("embed.json");
    std::fs::write(
        &config,
        r#"{"backend":"http","model":"fixture","revision":"fixed","url":"http://127.0.0.1:1/v1/embeddings","dimensions":2}"#,
    )?;
    // Empty corpus pins successful embed output without calling a real model.
    cli.record(
        "embed",
        &[
            "embed",
            "--config",
            config.to_str().ok_or("config path is not UTF-8")?,
        ],
        None,
        &[],
    )?;
    std::fs::write(
        &config,
        r#"{"backend":"http","model":"fixture","revision":"fixed","url":"bad-url","dimensions":2}"#,
    )?;
    cli.record(
        "embed-error",
        &[
            "embed",
            "--config",
            config.to_str().ok_or("config path is not UTF-8")?,
        ],
        None,
        &[],
    )?;
    cli.record(
        "serve-error",
        &[
            "serve",
            "--embedding-config",
            config.to_str().ok_or("config path is not UTF-8")?,
        ],
        None,
        &[],
    )?;
    Ok(())
}

fn record_serve_interrupt(cli: &Cli) -> TestResult {
    let socket = cli.root.path().join("comb.sock");
    let mut child = cli
        .command(&[
            "--socket",
            socket.to_str().ok_or("socket path is not UTF-8")?,
            "serve",
        ])
        .spawn()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::os::unix::net::UnixStream::connect(&socket).is_err() {
        assert!(child.try_wait()?.is_none());
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    // Signal immediately: spawning `kill` or sleeping can hide the bind/handler race.
    rustix::process::kill_process(
        rustix::process::Pid::from_child(&child),
        rustix::process::Signal::INT,
    )?;
    let output = child.wait_with_output()?;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty());
    golden(
        "serve.txt",
        format!(
            "exit={}\n{}",
            output.status.code().ok_or("child terminated by signal")?,
            String::from_utf8(output.stdout)?
        ),
    )?;
    Ok(())
}
