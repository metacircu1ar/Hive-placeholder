//! Invalid HTTP selectors must fail before defaults, settings, files or network.
use std::{net::TcpListener, path::Path, process::Command};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn invoke(root: &Path, url: &str, flag: &str, value: &str) -> Result {
    let mut command = Command::new(env!("CARGO_BIN_EXE_comb"));
    command
        .env_remove("HOME")
        .env("XDG_DATA_HOME", "invalid-relative")
        .arg("--root")
        .arg(root)
        .args(["model", "use-http"]);
    for (name, default) in [
        ("--url", url),
        ("--model", "PRIVATE-model"),
        ("--dimensions", "3"),
    ] {
        if name != flag {
            command.args([name, default]);
        }
    }
    let output = command.args([flag, value]).output()?;
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let expected = match flag {
        "--tokenizer" => "--max-tokens",
        "--max-tokens" => "--tokenizer",
        _ => flag,
    };
    assert_eq!(json["error"]["field"], expected);
    assert!(
        json["error"]["examples"]
            .as_array()
            .is_some_and(|v| !v.is_empty())
    );
    for text in [&output.stdout, &output.stderr] {
        assert!(!String::from_utf8_lossy(text).contains("PRIVATE"));
    }
    Ok(())
}

#[test]
fn invalid_http_options_precede_missing_or_corrupt_settings_and_endpoint_contact() -> Result {
    let directory = tempfile::tempdir()?;
    let missing = directory.path().join("missing");
    let corrupt = directory.path().join("corrupt");
    let admin = corrupt.join(".comb");
    std::fs::create_dir_all(&admin)?;
    let settings = admin.join("serve.json");
    std::fs::write(&settings, "PRIVATE corrupt settings")?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let url = format!("http://{}/v1/embeddings", listener.local_addr()?);
    for root in [&missing, &corrupt] {
        for (flag, value) in [
            ("--dimensions", "0"),
            ("--timeout-seconds", "121"),
            ("--model", ""),
            ("--revision", ""),
            ("--query-prefix", &"PRIVATE".repeat(147)),
            ("--document-prefix", &"PRIVATE".repeat(147)),
            ("--url", &format!("{url}?PRIVATE=secret")),
            ("--tokenizer", "PRIVATE-missing.json"),
            ("--max-tokens", "16"),
        ] {
            invoke(root, &url, flag, value)?;
        }
    }
    assert!(!missing.exists());
    assert_eq!(
        std::fs::read_to_string(settings)?,
        "PRIVATE corrupt settings"
    );
    assert_eq!(std::fs::read_dir(&admin)?.count(), 1);
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    Ok(())
}
