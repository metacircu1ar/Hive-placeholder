use super::*;
use crate::{Shutdown, stderr::StderrSink};
use tracing_subscriber::prelude::*;

#[test]
fn scoped_stderr_reports_cleanup_after_the_global_file_logger_has_closed() {
    if std::env::var_os("HIVE_STDERR_LATE_CHILD").is_some() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("daemon.jsonl");
        let (backend, guard) = crate::start(&path, Config::default()).unwrap();
        backend.install_global().unwrap();
        tracing::info!(name: "daemon.finished", count = 1);
        assert_eq!(
            guard.finish(Duration::from_secs(1)).state,
            Shutdown::Drained
        );
        let before = std::fs::read(&path).unwrap();
        let (late, guard) = crate::start_stderr(Config::default()).unwrap();
        late.scope(|| {
            let error = io::Error::other("private-late-cleanup-secret");
            tracing::warn!(name: "cleanup.late", error = &error as &(dyn std::error::Error + 'static), decision = "preserve_primary_failure");
        });
        assert_eq!(
            guard.finish(Duration::from_secs(1)).state,
            Shutdown::Drained
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "tests::stderr::scoped_stderr_reports_cleanup_after_the_global_file_logger_has_closed",
            "--nocapture",
        ])
        .env("HIVE_STDERR_LATE_CHILD", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(stderr.matches("cleanup.late").count(), 1);
    assert!(stderr.contains("preserve_primary_failure"));
    assert!(!stderr.contains("private-late-cleanup-secret"));
}

#[test]
fn standalone_human_diagnostics_preserve_only_sanitized_fields() {
    let output = Memory::default();
    let (logger, guard) = start_with(
        Config::default(),
        Box::new(StderrSink(Box::new(output.clone()))),
        Box::new(io::sink()),
        &Threads,
    )
    .unwrap();
    let backend = crate::subscriber::from_logger(logger);
    tracing::subscriber::with_default(tracing_subscriber::registry().with(backend), || {
        let error = io::Error::other("private-body-secret-account");
        tracing::warn!(name: "cleanup.failed", error = &error as &(dyn std::error::Error + 'static), decision = "retain_evidence", pid = 42_u32);
    });
    assert_eq!(
        guard.finish(Duration::from_secs(2)).state,
        Shutdown::Drained
    );
    let data = output.0.lock().unwrap().concat();
    let text = String::from_utf8(data).unwrap();
    assert!(text.starts_with("Hive warn cleanup.failed "));
    assert!(text.contains("decision=\"retain_evidence\""));
    assert!(text.contains("\"kind\":\"io\""));
    assert!(!text.contains("private-body-secret-account"));
    assert_eq!(text.lines().count(), 1);
}

#[test]
fn standalone_blocked_stderr_does_not_hold_admission_or_shutdown() {
    let (output, entered, release) = Blocked::new(Memory::default());
    let (logger, guard) = start_with(
        Config {
            queue_slots: 1,
            ..Config::default()
        },
        Box::new(StderrSink(Box::new(output))),
        Box::new(io::sink()),
        &Threads,
    )
    .unwrap();
    let backend = crate::subscriber::from_logger(logger);
    tracing::subscriber::with_default(tracing_subscriber::registry().with(backend.clone()), || {
        tracing::info!(name: "stderr.block", count = 1);
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        tracing::info!(name: "stderr.queued", count = 1);
        tracing::info!(name: "stderr.overflow", count = 1);
    });
    assert_eq!(backend.counts().full, 1);
    let began = Instant::now();
    assert_eq!(
        guard.finish(Duration::from_millis(25)).state,
        Shutdown::Unconfirmed
    );
    assert!(began.elapsed() < Duration::from_millis(500));
    release.send(()).unwrap();
}

#[test]
fn blocked_inherited_stderr_does_not_lock_main_thread_or_delay_process_exit() {
    use std::{
        os::fd::OwnedFd,
        os::unix::net::UnixStream,
        process::{Command, Stdio},
    };
    if std::env::var_os("HIVE_STDERR_BLOCKED_CHILD").is_some() {
        let (backend, guard) = crate::start_stderr(Config::default()).unwrap();
        backend.install_global().unwrap();
        tracing::warn!(name: "stderr.blocked_native", count = 1);
        let began = Instant::now();
        assert_eq!(
            guard.finish(Duration::from_millis(100)).state,
            Shutdown::Unconfirmed
        );
        assert!(began.elapsed() < Duration::from_secs(1));
        // Diagnostic I/O must not keep this global stdio lock: result emission
        // takes it even when there is no primary human error to print.
        drop(std::io::stderr().lock());
        return;
    }
    let (_reader, mut writer) = UnixStream::pair().unwrap();
    writer.set_nonblocking(true).unwrap();
    let fill = vec![0_u8; 65536];
    loop {
        match writer.write(&fill) {
            Ok(_) => (),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) => panic!("fill inherited stderr: {error}"),
        }
    }
    writer.set_nonblocking(false).unwrap();
    let fd: OwnedFd = writer.into();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "tests::stderr::blocked_inherited_stderr_does_not_lock_main_thread_or_delay_process_exit", "--nocapture"])
        .env("HIVE_STDERR_BLOCKED_CHILD", "1")
        .stdout(Stdio::null()).stderr(Stdio::from(fd)).spawn().unwrap();
    let began = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if began.elapsed() > Duration::from_secs(5) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("blocked diagnostics held process exit");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success());
}

#[test]
fn writer_diagnostics_are_human_lines_in_the_given_writer() {
    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<Vec<u8>>>);
    impl io::Write for Shared {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let output = Shared::default();
    let (backend, guard) = crate::start_writer(Config::default(), output.clone()).unwrap();
    backend.scope(|| {
        tracing::warn!(name: "cleanup.failed", decision = "retain_evidence");
    });
    assert_eq!(
        guard.finish(Duration::from_secs(1)).state,
        Shutdown::Drained
    );
    let text = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    assert!(text.starts_with("Hive warn cleanup.failed"), "{text}");
    assert!(text.contains("retain_evidence"), "{text}");
}
