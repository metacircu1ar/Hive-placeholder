use super::{Engine, read_bounded};
use crate::{Result, embedding::Config};
use std::{
    os::unix::{fs::symlink, net::UnixListener},
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const PROBE: &str = "COMB_TEST_ASSET_PROBE";
const TEST: &str =
    "embedding::backend::assets_tests::special_assets_fail_without_waiting_for_a_writer";

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let cleanup = (|| -> std::io::Result<()> {
            if self.0.try_wait()?.is_none() {
                self.0.kill()?;
            }
            self.0.wait()?;
            Ok(())
        })();
        if !std::thread::panicking() {
            assert!(cleanup.is_ok(), "asset fixture cleanup failed: {cleanup:?}");
        }
    }
}

#[test]
fn special_assets_fail_without_waiting_for_a_writer() -> Result<()> {
    if let Some(path) = std::env::var_os(PROBE) {
        probe(Path::new(&path));
        return Ok(());
    }
    let directory = tempfile::tempdir()?;
    let fifo = directory.path().join("PRIVATE.fifo");
    let native = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: native is a live NUL-terminated path; the FIFO has private permissions.
    assert_eq!(unsafe { libc::mkfifo(native.as_ptr(), 0o600) }, 0);
    let linked = directory.path().join("linked");
    symlink(&fifo, &linked)?;
    for path in [&fifo, &linked, directory.path(), Path::new("/dev/null")] {
        let mut child = Process(
            Command::new(std::env::current_exe()?)
                .args(["--exact", TEST, "--nocapture"])
                .env(PROBE, path)
                .stdin(Stdio::null())
                .spawn()?,
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.0.try_wait()? {
                assert!(status.success(), "asset probe failed: {path:?}");
                break;
            }
            assert!(Instant::now() < deadline, "asset open blocked: {path:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    Ok(())
}

fn probe(path: &Path) {
    assert_asset_error(read_bounded(path, 1024).err());
    let config = http(path);
    assert_asset_error(Engine::http_identity(config.clone()).err());
    assert_asset_error(Engine::load(Config::Http(config)).err());
}

fn assert_asset_error(error: Option<crate::Error>) {
    let error = error.expect("special asset must fail");
    assert_eq!(error.code.as_str(), "invalid_embedding_asset");
    assert_eq!(
        error.message,
        "Embedding assets must be readable regular files."
    );
    assert!(!serde_json::to_string(&error).unwrap().contains("PRIVATE"));
}

fn http(path: &Path) -> crate::embedding::HttpConfig {
    serde_json::from_value(serde_json::json!({
        "model":"fixture", "revision":"fixed", "dimensions":3,
        "url":"http://127.0.0.1:9/v1/embeddings", "tokenizer":path, "max_tokens":32
    }))
    .unwrap()
}

#[test]
fn inaccessible_assets_preserve_native_io_errors() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let missing = directory.path().join("missing");
    let dangling = directory.path().join("dangling");
    symlink(&missing, &dangling)?;
    let socket = directory.path().join("socket");
    let _listener = UnixListener::bind(&socket)?;
    for path in [&missing, &dangling, &socket] {
        let error = read_bounded(path, 1).unwrap_err();
        assert_eq!(error.code, crate::ErrorCode::IoError);
        assert!(std::error::Error::source(&error).is_some());
    }
    Ok(())
}
