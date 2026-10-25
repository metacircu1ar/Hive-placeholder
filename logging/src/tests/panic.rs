use super::*;
use crate::{Admission, Level, Shutdown};
use std::{process::Command, thread};
struct PanicSink;
impl Sink for PanicSink {
    fn write(&mut self, _: &[u8]) -> io::Result<()> {
        panic!("sk-secret-writer-payload");
    }
    fn finish(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[test]
fn panic_hooks_are_sanitized_and_writer_panic_uses_fallback() {
    if std::env::var_os("HIVE_LOG_PANIC_CHILD").is_some() {
        let stderr = Memory::default();
        let (logger, guard) = start_with(
            Config::default(),
            Box::new(PanicSink),
            Box::new(stderr.clone()),
            &Threads,
        )
        .unwrap();
        let prior_ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let prior_flag = prior_ran.clone();
        std::panic::set_hook(Box::new(move |_| {
            prior_flag.store(true, std::sync::atomic::Ordering::Release);
        }));
        crate::panic::install_panic_hook(logger.clone());
        logger.record(Level::Info, "trigger", |_| Ok(()));
        until(|| !stderr.records().is_empty());
        assert_eq!(
            guard.finish(Duration::from_secs(1)).state,
            Shutdown::Panicked
        );
        assert!(prior_ran.load(std::sync::atomic::Ordering::Acquire));
        let records = stderr.records();
        assert_eq!(records[0]["event"], "process.panic");
        assert_eq!(records[0]["thread"], "log_writer");
        assert_eq!(records[0]["payload"], "string");
        assert!(
            !serde_json::to_string(&records)
                .unwrap()
                .contains("sk-secret")
        );
        assert_eq!(
            logger.record(Level::Info, "disconnected", |_| Ok(())),
            Admission::Closed
        );
        // A fresh logger proves startup isn't permanently poisoned by the failure.
        let (logger, guard, memory) = start_memory(Config::default());
        crate::panic::install_panic_hook(logger);
        let result = thread::Builder::new()
            .name("sk-secret-thread-name".into())
            .spawn(|| panic!("sk-secret-user-payload"))
            .unwrap()
            .join();
        assert!(result.is_err());
        assert_eq!(
            guard.finish(Duration::from_secs(1)).state,
            Shutdown::Drained
        );
        let text = serde_json::to_string(&memory.records()).unwrap();
        assert!(!text.contains("sk-secret"));
        assert!(text.contains("application"));

        caught_panic_with_blocked_sinks_is_counted();
        return;
    }
    let result = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "tests::panic::panic_hooks_are_sanitized_and_writer_panic_uses_fallback",
            "--nocapture",
        ])
        .env("HIVE_LOG_PANIC_CHILD", "1")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

fn caught_panic_with_blocked_sinks_is_counted() {
    let (sink, entered, release) = Blocked::new(Memory::default());
    let (stderr, stderr_entered, stderr_release) = Blocked::new(Memory::default());
    let (logger, guard) = start_with(
        Config::default(),
        Box::new(sink),
        Box::new(stderr),
        &Threads,
    )
    .unwrap();
    // Use the public control path, and isolate this check from previous hooks.
    std::panic::set_hook(Box::new(|_| {}));
    crate::subscriber::from_logger(logger.clone()).install_panic_hook();
    logger.record(Level::Info, "block", |_| Ok(()));
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    let start = Instant::now();
    assert!(std::panic::catch_unwind(|| panic!("sk-secret-caught-panic")).is_err());
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(logger.counts().fatal_unconfirmed, 1);
    stderr_entered.recv_timeout(Duration::from_secs(1)).unwrap();
    release.send(()).unwrap();
    stderr_release.send(()).unwrap();
    assert_eq!(
        guard.finish(Duration::from_secs(1)).state,
        Shutdown::Drained
    );
}
