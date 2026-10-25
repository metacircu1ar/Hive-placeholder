#![allow(clippy::unwrap_used)] // Bounded input fixtures fail loudly on unexpected results.
use super::*;

#[test]
fn byte_limit_precedes_utf8_decoding_and_empty_policy_is_exact() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("payload");
    let context = Context::new("spawn", "--body-file").file(&path);
    std::fs::write(&path, [b'a', 0xff]).unwrap();
    assert_eq!(
        context.text(1, false).unwrap_err().error.code(),
        "out_of_range"
    );
    assert_eq!(
        context.text(2, false).unwrap_err().error.code(),
        "invalid_file"
    );
    std::fs::write(&path, "").unwrap();
    assert_eq!(
        context.text(10, false).unwrap_err().error.code(),
        "invalid_value"
    );
    assert_eq!(context.text(10, true).unwrap(), "");
    std::fs::write(&path, " \t\n").unwrap();
    assert_eq!(context.text(10, false).unwrap(), " \t\n");
}

#[test]
fn inherited_blocking_pipes_keep_flags_and_exact_bytes_on_success_or_timeout() {
    use std::{io::Write, os::fd::AsRawFd, time::Duration};
    for close in [false, true] {
        let (input, mut writer) = std::io::pipe().unwrap();
        let path = std::path::PathBuf::from(format!("/dev/fd/{}", input.as_raw_fd()));
        let before = rustix::fs::fcntl_getfl(&input).unwrap();
        assert!(!before.contains(rustix::fs::OFlags::NONBLOCK));
        writer.write_all(b"exact\n\xffpipe").unwrap();
        let writer = if close {
            drop(writer);
            None
        } else {
            Some(writer)
        };
        let result = reader::read(&path, 100, Duration::from_millis(80));
        assert_eq!(rustix::fs::fcntl_getfl(&input).unwrap(), before);
        if close {
            assert_eq!(result.unwrap(), b"exact\n\xffpipe");
        } else {
            assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
        }
        drop(writer);
    }
}

#[test]
fn pipe_cap_and_total_deadline_cannot_be_extended_by_progress() {
    use std::{
        io::Write,
        os::fd::AsRawFd,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::{Duration, Instant},
    };
    let (input, mut writer) = std::io::pipe().unwrap();
    let path = std::path::PathBuf::from(format!("/dev/fd/{}", input.as_raw_fd()));
    writer.write_all(&[b'x'; 17]).unwrap();
    assert_eq!(
        reader::read(&path, 16, Duration::from_secs(1))
            .unwrap()
            .len(),
        17
    );
    let stop = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&stop);
    let worker = std::thread::spawn(move || {
        while !signal.load(Ordering::Relaxed) {
            writer.write_all(b"x").unwrap();
            std::thread::sleep(Duration::from_millis(5));
        }
    });
    let start = Instant::now();
    let result = reader::read(&path, 10000, Duration::from_millis(80));
    stop.store(true, Ordering::Relaxed);
    worker.join().unwrap();
    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn unattached_fifo_does_not_masquerade_as_empty_instructions() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("pipe");
    let native = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    // SAFETY: native is a live NUL-terminated path in an owned private test directory.
    assert_eq!(unsafe { libc::mkfifo(native.as_ptr(), 0o600) }, 0);
    let result = reader::read(&path, 100, std::time::Duration::from_millis(40));
    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
}

#[test]
fn completed_fifo_does_not_wait_for_a_missing_macos_hangup_event() {
    use std::io::Write;

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("pipe");
    let native = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    // SAFETY: native is a live NUL-terminated path in a private test directory.
    assert_eq!(unsafe { libc::mkfifo(native.as_ptr(), 0o600) }, 0);
    let child_path = path.clone();
    let writer = std::thread::spawn(move || {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(child_path)
            .unwrap();
        file.write_all(b"completed").unwrap();
    });
    let result = reader::read(&path, 100, std::time::Duration::from_secs(2));
    writer.join().unwrap();
    assert_eq!(result.unwrap(), b"completed");
}

#[test]
fn source_causes_are_retained_and_private_paths_never_enter_error_metadata() {
    use std::os::unix::ffi::OsStrExt;
    let path = Path::new(std::ffi::OsStr::from_bytes(b"PRIVATE\n\xff"));
    let context = Context::new("send", "--body-file").file(path);
    for (errno, expected) in [
        (libc::ENOENT, "invalid_file"),
        (libc::EMFILE, "state_io"),
        (libc::EIO, "state_io"),
    ] {
        let failure = context.io(std::io::Error::from_raw_os_error(errno));
        assert_eq!(failure.error.code(), expected);
        assert_eq!(failure.exit_code, if errno == libc::ENOENT { 2 } else { 1 });
        assert_eq!(failure.error.io_raw_os_error(), Some(errno));
        let wire = serde_json::to_string(&failure.error).unwrap();
        assert!(!wire.contains("PRIVATE"));
        assert!(!format!("{failure:?}").contains("PRIVATE"));
        let line = failure.path_line().unwrap();
        assert!(line.contains("PRIVATE\\n\\xFF"), "{line}");
        assert_eq!(line.matches('\n').count(), 1);
    }
    let long = std::path::PathBuf::from("ü".repeat(500));
    assert!(
        Context::new("send", "--body-file")
            .file(&long)
            .io(std::io::ErrorKind::NotFound.into())
            .path_line()
            .unwrap()
            .len()
            <= 207
    );
}

#[test]
fn prepared_dispatch_uses_original_file_bytes_after_deletion() {
    use clap::Parser;
    let stub = super::super::response_tests::Stub::new(vec![serde_json::json!({"message_id":"m"})]);
    let file = stub.root().join("body");
    std::fs::write(&file, "original\n💡").unwrap();
    let cli = super::super::Cli::try_parse_from([
        "hivectl",
        "--config",
        "/unused",
        "--state-dir",
        stub.root().to_str().unwrap(),
        "send",
        "agent",
        "--body-file",
        file.to_str().unwrap(),
    ])
    .unwrap();
    let prepared = super::super::dispatch::Prepared::new(cli).unwrap();
    std::fs::remove_file(&file).unwrap();
    assert!(super::super::dispatch::execute(prepared).unwrap().success);
    assert_eq!(stub.finish()[0]["args"]["body"], "original\n💡");
}

#[test]
fn shared_caps_and_arbitrary_approval_json_are_preserved() {
    use clap::Parser;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("content");
    for cap in [
        super::super::BODY_BYTES,
        crate::approvals::RESPONSE_BYTES,
        super::super::BATCH_BYTES,
        8192,
    ] {
        let mut text = "💡".repeat(cap / 4);
        text.push_str(&"x".repeat(cap % 4));
        std::fs::write(&path, &text).unwrap();
        let context = Context::new("send", "--body-file").file(&path);
        assert_eq!(context.text(cap, false).unwrap(), text);
        text.push('x');
        std::fs::write(&path, text).unwrap();
        assert_eq!(
            context.text(cap, false).unwrap_err().error.code(),
            "out_of_range"
        );
    }
    for text in ["null", "true", "17", "[]", "{}", "\"exact text\""] {
        std::fs::write(&path, text).unwrap();
        for option in ["--data", "--data-file"] {
            let value = if option == "--data" {
                text
            } else {
                path.to_str().unwrap()
            };
            let cli = super::super::Cli::try_parse_from([
                "hivectl",
                "respond",
                "00000000-0000-4000-8000-000000000001",
                "--accept",
                option,
                value,
            ])
            .unwrap();
            let super::super::Command::Respond(args) = cli.command else {
                panic!("respond");
            };
            let request = super::super::approvals::response_request(&args).unwrap();
            let encoded = serde_json::to_value(request).unwrap();
            assert_eq!(
                encoded["content"],
                serde_json::from_str::<serde_json::Value>(text).unwrap()
            );
            assert!(encoded.get("content").is_some());
        }
    }
}

#[test]
fn inline_messages_preserve_exact_bytes_and_control_keys_reach_dispatch() {
    use clap::Parser;
    let capped = format!(" \n{} \t", "💡".repeat((super::super::BODY_BYTES - 4) / 4));
    assert_eq!(capped.len(), super::super::BODY_BYTES);
    for command in ["spawn", "send", "complete"] {
        for text in [capped.as_str(), "- item", "---\ntitle", "--not-a-flag"] {
            let stub = super::super::response_tests::Stub::new(vec![serde_json::json!({})]);
            let mut argv = vec![
                "hivectl",
                "--config",
                "/unused",
                "--state-dir",
                stub.root().to_str().unwrap(),
                command,
                "agent",
                "--body",
                text,
                "--request-id",
                "same-key",
            ];
            if command == "spawn" {
                argv.extend(["--cwd", "/tmp"]);
            }
            let cli = super::super::Cli::try_parse_from(argv).unwrap();
            let prepared = super::super::dispatch::Prepared::new(cli).unwrap();
            assert!(super::super::dispatch::execute(prepared).unwrap().success);
            let sent = stub.finish();
            assert_eq!(sent[0]["id"], "same-key");
            assert_eq!(sent[0]["args"]["body"], text);
            assert_eq!(sent[0]["args"]["op"], command);
        }
    }
    for command in ["stop", "resume", "interrupt", "terminate"] {
        let stub = super::super::response_tests::Stub::new(vec![serde_json::json!({})]);
        let cli = super::super::Cli::try_parse_from([
            "hivectl",
            "--config",
            "/unused",
            "--state-dir",
            stub.root().to_str().unwrap(),
            command,
            "agent",
            "--request-id",
            "preserved-key",
        ])
        .unwrap();
        let prepared = super::super::dispatch::Prepared::new(cli).unwrap();
        assert!(super::super::dispatch::execute(prepared).unwrap().success);
        let sent = stub.finish();
        assert_eq!(sent[0]["id"], "preserved-key");
        assert_eq!(
            sent[0]["args"],
            serde_json::json!({"op":command,"target":"agent"})
        );
    }
}

#[test]
fn directory_preparation_keeps_absolute_spelling_and_rejects_private_invalid_paths() {
    use clap::Parser;
    use std::os::unix::ffi::OsStrExt;
    let relative = Path::new(".");
    assert_eq!(
        Context::new("spawn", "--cwd")
            .file(relative)
            .directory()
            .unwrap(),
        std::path::absolute(relative).unwrap()
    );
    let temp = tempfile::tempdir().unwrap();
    let link = temp.path().join("link");
    std::os::unix::fs::symlink(temp.path(), &link).unwrap();
    assert_eq!(
        Context::new("spawn", "--cwd")
            .file(&link)
            .directory()
            .unwrap(),
        link
    );
    let cli =
        super::super::Cli::try_parse_from(["hivectl", "spawn", "--cwd", ".", "--body", "work"])
            .unwrap();
    let super::super::Command::Spawn(args) = cli.command else {
        panic!("spawn");
    };
    assert_eq!(
        serde_json::to_value(args.request().unwrap()).unwrap()["cwd"],
        serde_json::json!(std::path::absolute(".").unwrap())
    );
    let selected = Path::new("relative");
    let context = Context::new("spawn", "--cwd").file(selected);
    let invalid = std::path::PathBuf::from(std::ffi::OsStr::from_bytes(b"/PRIVATE\xff/relative"));
    let failure = context.resolved_directory(invalid).unwrap_err();
    assert_eq!(failure.error.code(), "invalid_path");
    assert_eq!(
        failure.error.message(),
        "Working directory must be valid UTF-8."
    );
    assert_eq!(failure.path.as_deref(), Some(selected));
    assert!(
        !serde_json::to_string(&failure.error)
            .unwrap()
            .contains("PRIVATE")
    );
    assert!(!format!("{failure:?}").contains("PRIVATE"));
    for (errno, code, exit) in [
        (libc::ENOENT, "invalid_path", 2),
        (libc::EACCES, "invalid_path", 2),
        (libc::EIO, "state_io", 1),
        (libc::EMFILE, "state_io", 1),
    ] {
        let failure = context.directory_io(std::io::Error::from_raw_os_error(errno));
        assert_eq!(failure.error.code(), code);
        assert_eq!(failure.exit_code, exit);
        assert_eq!(failure.error.io_raw_os_error(), Some(errno));
    }
}

#[test]
fn branch_preparation_keeps_omitted_directory_and_lifetime_missing() {
    use clap::Parser;
    let cli = super::super::Cli::try_parse_from([
        "hivectl", "spawn", "--from", "source", "--body", "work",
    ])
    .unwrap();
    let super::super::Command::Spawn(args) = cli.command else {
        panic!("spawn");
    };
    let request = serde_json::to_value(args.request().unwrap()).unwrap();
    assert!(request.get("cwd").is_none());
    assert!(request.get("lifetime").is_none());
    assert_eq!(request["from_agent"], "source");
}
