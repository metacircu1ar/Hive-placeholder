use super::{Context, reader};
use comb::page::PAGE_BYTES;
use std::{
    io::{self, Write},
    os::unix::{
        ffi::OsStringExt,
        fs::{PermissionsExt, symlink},
    },
    path::PathBuf,
    time::{Duration, Instant},
};

#[test]
fn ordinary_inputs_keep_symlink_shared_access_and_exact_text() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("input.md");
    let link = directory.path().join("linked.md");
    let markdown = "---\ntitle: 日本語\n---\n  Exact text.\n";
    std::fs::write(&target, markdown).unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o666)).unwrap();
    symlink(&target, &link).unwrap();
    assert_eq!(
        Context::new("put", "--file")
            .file(&link)
            .markdown()
            .unwrap(),
        markdown
    );
    std::fs::write(&target, "").unwrap();
    assert_eq!(
        Context::new("append", "--summary-file")
            .file(&link)
            .summary(false)
            .unwrap(),
        ""
    );
    assert_eq!(
        Context::new("consolidate", "--summary-file")
            .file(&link)
            .summary(true)
            .unwrap_err()
            .code(),
        "invalid_value"
    );
    std::fs::write(&target, "text\0tail").unwrap();
    assert_eq!(
        Context::new("append", "--summary-file")
            .file(&link)
            .summary(false)
            .unwrap_err()
            .code(),
        "invalid_value"
    );
}

#[test]
fn byte_limit_precedes_decode_and_the_reader_stops_after_one_probe_byte() {
    let context = Context::new("put", "--file");
    assert_eq!(
        context.decode(vec![b'a'; PAGE_BYTES]).unwrap().len(),
        PAGE_BYTES
    );
    let mut bytes = vec![b'a'; PAGE_BYTES + 1];
    bytes[0] = 0xff;
    assert_eq!(context.decode(bytes).unwrap_err().code(), "out_of_range");
    assert_eq!(
        context.decode(vec![0xff]).unwrap_err().code(),
        "invalid_file"
    );
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("large");
    std::fs::write(&file, "éééé").unwrap();
    let bytes = reader::read(&file, 2, reader::PIPE_DEADLINE).unwrap();
    assert_eq!(bytes, "éé".as_bytes()[..3]);
    assert!(reader::read(dir.path(), 2, reader::PIPE_DEADLINE).is_err());
}

#[test]
fn selected_path_is_escaped_local_only_and_native_resource_errors_stay_operational() {
    let path = PathBuf::from("PRIVATE\n\x1b[2J");
    let context = Context::new("put", "--file").file(&path);
    for kind in [
        io::ErrorKind::NotFound,
        io::ErrorKind::PermissionDenied,
        io::ErrorKind::TimedOut,
    ] {
        let output = context
            .io(io::Error::new(kind, "PAYLOAD_SENTINEL"))
            .render();
        assert_eq!(output.exit_code, 2);
        assert!(!output.stdout.contains("PRIVATE"));
        assert!(output.stderr.contains("Path: PRIVATE\\n\\x1b"));
        assert!(!output.stderr.contains('\x1b'));
        assert!(!output.stderr.contains("PAYLOAD_SENTINEL"));
    }
    let output = context.io(io::Error::from_raw_os_error(libc::EIO)).render();
    assert_eq!(output.exit_code, 1);
    assert!(output.stdout.contains("io_error"));
    assert!(!output.stdout.contains("PRIVATE"));
    assert!(output.stderr.contains("Path: PRIVATE"));
}

#[test]
fn import_requires_an_encodable_canonical_directory() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("ordinary");
    std::fs::write(&file, "text").unwrap();
    assert_eq!(
        Context::new("import", "DIR")
            .file(&file)
            .directory()
            .unwrap_err()
            .code(),
        "invalid_path"
    );
    assert_eq!(
        Context::new("import", "DIR")
            .file(dir.path())
            .directory()
            .unwrap(),
        dir.path().canonicalize().unwrap()
    );
    let invalid = PathBuf::from(std::ffi::OsString::from_vec(
        b"/tmp/PRIVATE-\xff/notes".to_vec(),
    ));
    let context = Context::new("import", "DIR").file(std::path::Path::new("notes"));
    let output = context.resolved_path(&invalid).unwrap_err().render();
    assert_eq!(output.exit_code, 2);
    assert!(!output.stdout.contains("PRIVATE"));
    assert!(!output.stderr.contains("PRIVATE")); // Show the selected spelling, not its private ancestor.
    assert!(output.stderr.contains("Path: notes"));
    assert_eq!(
        Context::new("import", "DIR")
            .file(&invalid)
            .directory()
            .unwrap_err()
            .code(),
        "invalid_path"
    );
}

#[test]
fn fifo_readiness_eof_and_deadline_are_bounded() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pipe");
    let native = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: native is a live NUL-terminated path; mode creates a private FIFO.
    assert_eq!(unsafe { libc::mkfifo(native.as_ptr(), 0o600) }, 0);
    let start = Instant::now();
    let error = reader::read(&path, 32, Duration::from_millis(20)).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(start.elapsed() < Duration::from_secs(2));
    let child_path = path.clone();
    let writer = std::thread::spawn(move || {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(child_path)
            .unwrap();
        std::thread::sleep(Duration::from_millis(10));
        file.write_all(b"first").unwrap();
        file.write_all(b"second").unwrap();
    });
    assert_eq!(
        reader::read(&path, 32, Duration::from_secs(2)).unwrap(),
        b"firstsecond"
    );
    writer.join().unwrap();
}

#[test]
fn remapped_import_input_error_keeps_progress_and_nonzero_exit() {
    let mut error = comb_protocol::service::input::admin::Admin::Import
        .error(hive_input::Code::InvalidValue, "directory")
        .committed();
    error.import_progress = Some(Box::new(comb_protocol::import::ImportProgress {
        batch_id: comb::values::ImportId::generate(),
        state: comb_protocol::import::ImportState::Pending,
        total: 2,
        completed: 1,
        unfinished: 1,
        unresolved: None,
    }));
    let expected = error.import_progress.clone();
    let rendered = super::Failure::remote(error, "import", None).render();
    assert_ne!(rendered.exit_code, 0);
    let wire: serde_json::Value = serde_json::from_str(&rendered.stdout).unwrap();
    let restored: comb::Error = serde_json::from_value(wire["error"].clone()).unwrap();
    assert!(restored.file_committed);
    assert_eq!(restored.input().unwrap().field(), "DIR");
    assert_eq!(restored.import_progress, expected);
}
