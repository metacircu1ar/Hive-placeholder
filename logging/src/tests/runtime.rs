use super::*;
use crate::{Admission, Level, Receipt, Shutdown};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread,
};

#[test]
fn full_queue_reports_loss_while_idle_and_fatal_bypasses_it() {
    let memory = Memory::default();
    let (sink, entered, release) = Blocked::new(memory.clone());
    let stderr = Memory::default();
    let config = Config {
        queue_slots: 1,
        loss_interval: Duration::from_millis(10),
        ..Config::default()
    };
    let (logger, guard) =
        start_with(config, Box::new(sink), Box::new(stderr.clone()), &Threads).unwrap();
    assert_eq!(
        logger.record(Level::Info, "one", |_| Ok(())),
        Admission::Queued
    );
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(
        logger.record(Level::Info, "two", |_| Ok(())),
        Admission::Queued
    );
    assert_eq!(
        logger.record(Level::Info, "three", |_| Ok(())),
        Admission::Full
    );
    let started = Instant::now();
    let receipt = logger.fatal("fatal.test", |_| Ok(()));
    assert!(matches!(
        receipt,
        Receipt::StderrConfirmed | Receipt::Unconfirmed
    ));
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(
        logger.counts().fatal_unconfirmed,
        u64::from(receipt == Receipt::Unconfirmed)
    );
    until(|| !stderr.records().is_empty());
    assert_eq!(stderr.records()[0]["event"], "fatal.test");
    release.send(()).unwrap();
    until(|| memory.records().iter().any(|r| r["event"] == "log.dropped"));
    let records = memory.records();
    let fatal = records
        .iter()
        .position(|r| r["event"] == "fatal.test")
        .unwrap();
    let two = records.iter().position(|r| r["event"] == "two").unwrap();
    assert!(fatal < two);
    assert_eq!(logger.counts().full, 1);
    assert_eq!(
        guard.finish(Duration::from_secs(2)).state,
        Shutdown::Drained
    );
}
#[test]
fn blocked_disk_and_stderr_cannot_hold_fatal_or_shutdown() {
    let (sink, entered, release) = Blocked::new(Memory::default());
    let (stderr, stderr_entered, stderr_release) = Blocked::new(Memory::default());
    let (logger, guard) = start_with(
        Config::default(),
        Box::new(sink),
        Box::new(stderr),
        &Threads,
    )
    .unwrap();
    logger.record(Level::Info, "block", |_| Ok(()));
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    let start = Instant::now();
    assert_eq!(logger.fatal("fatal.test", |_| Ok(())), Receipt::Unconfirmed);
    assert!(start.elapsed() < Duration::from_secs(1));
    stderr_entered.recv_timeout(Duration::from_secs(1)).unwrap();
    let start = Instant::now();
    assert_eq!(
        guard.finish(Duration::from_millis(25)).state,
        Shutdown::Unconfirmed
    );
    assert!(start.elapsed() < Duration::from_millis(500));
    assert_eq!(
        logger.record(Level::Info, "late", |_| Ok(())),
        Admission::Closed
    );
    release.send(()).unwrap();
    stderr_release.send(()).unwrap();
}
struct FailOnce {
    memory: Memory,
    failed: bool,
}
impl Sink for FailOnce {
    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        if !self.failed {
            self.failed = true;
            return Err(io::Error::from_raw_os_error(28));
        }
        Sink::write(&mut self.memory, bytes)
    }
    fn finish(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[test]
fn failure_count_survives_failed_loss_receipts_and_recovers_when_idle() {
    let memory = Memory::default();
    let stderr = Memory::default();
    let sink = FailOnce {
        memory: memory.clone(),
        failed: false,
    };
    let (logger, guard) = start_with(
        Config {
            loss_interval: Duration::from_millis(10),
            ..Config::default()
        },
        Box::new(sink),
        Box::new(stderr.clone()),
        &Threads,
    )
    .unwrap();
    logger.record(Level::Info, "lost.once", |_| Ok(()));
    until(|| logger.counts().write_failed > 0);
    until(|| memory.records().iter().any(|r| r["event"] == "log.dropped"));
    assert_eq!(memory.records()[0]["write_failed"], 1);
    assert_eq!(logger.counts().last_errno, Some(28));
    assert_eq!(stderr.records().len(), 1);
    assert!(!memory.records().iter().any(|r| r["event"] == "lost.once"));
    assert_eq!(
        guard.finish(Duration::from_secs(2)).state,
        Shutdown::WriteFailed
    );
}
#[test]
fn finish_drains_accepted_records_with_live_racing_clones() {
    let (logger, guard, memory) = start_memory(Config::default());
    let run = Arc::new(AtomicBool::new(true));
    let mut writers = Vec::new();
    for _ in 0..8 {
        let logger = logger.clone();
        let run = run.clone();
        writers.push(thread::spawn(move || {
            let mut accepted = 0;
            while run.load(Ordering::Relaxed) {
                if logger.record(Level::Info, "accepted", |_| Ok(())) == Admission::Queued {
                    accepted += 1;
                }
                thread::yield_now();
            }
            accepted
        }));
    }
    until(|| logger.counts().written >= 20);
    let finished = guard.finish(Duration::from_secs(5));
    run.store(false, Ordering::Relaxed);
    let accepted: usize = writers.into_iter().map(|w| w.join().unwrap()).sum();
    assert_eq!(finished.state, Shutdown::Drained);
    assert_eq!(
        memory
            .records()
            .iter()
            .filter(|r| r["event"] == "accepted")
            .count(),
        accepted
    );
    assert_eq!(
        logger.record(Level::Error, "late", |_| Ok(())),
        Admission::Closed
    );
}
struct FailThread(&'static str);
impl crate::Spawn for FailThread {
    fn spawn(
        &self,
        name: &'static str,
        task: Box<dyn FnOnce() + Send>,
    ) -> io::Result<thread::JoinHandle<()>> {
        if name == self.0 {
            Err(io::Error::other("injected thread failure"))
        } else {
            Threads.spawn(name, task)
        }
    }
}
#[test]
fn either_thread_start_failure_can_be_retried() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log.jsonl");
    for name in ["hive-log-writer", "hive-log-stderr"] {
        let sink = crate::sink::FileSink::open(&path, &Config::default()).unwrap();
        assert!(
            start_with(
                Config::default(),
                Box::new(sink),
                Box::new(io::sink()),
                &FailThread(name)
            )
            .is_err()
        );
        let (logger, guard) = crate::guard::start(&path, Config::default()).unwrap();
        assert_eq!(
            logger.fatal("startup.retry", |_| Ok(())),
            Receipt::DiskConfirmed
        );
        assert_eq!(
            guard.finish(Duration::from_secs(2)).state,
            Shutdown::Drained
        );
    }
}

struct FailedFinish;
impl Sink for FailedFinish {
    fn write(&mut self, _: &[u8]) -> io::Result<()> {
        Ok(())
    }
    fn finish(&mut self) -> io::Result<()> {
        Err(io::Error::from_raw_os_error(28))
    }
}
#[test]
fn finish_failure_is_distinct_from_a_lost_record() {
    let stderr = Memory::default();
    let (blocked, entered, release) = Blocked::new(stderr.clone());
    let (logger, guard) = start_with(
        Config::default(),
        Box::new(FailedFinish),
        Box::new(blocked),
        &Threads,
    )
    .unwrap();
    assert_eq!(
        logger.record(Level::Info, "accepted", |_| Ok(())),
        Admission::Queued
    );
    let started = Instant::now();
    let result = guard.finish(Duration::from_secs(1));
    assert!(started.elapsed() < Duration::from_secs(1));
    entered.recv_timeout(Duration::from_secs(1)).unwrap();
    release.send(()).unwrap();
    until(|| {
        stderr
            .records()
            .iter()
            .any(|r| r["event"] == "log.finish_failed")
    });
    assert_eq!(result.state, Shutdown::WriteFailed);
    assert_eq!(result.counts.written, 1);
    assert_eq!(result.counts.write_failed, 0);
    assert_eq!(result.counts.finish_failed, 1);
}

#[test]
fn idle_threads_wake_only_for_receipt_deadlines_and_notifications() {
    let config = Config {
        loss_interval: Duration::from_millis(100),
        ..Config::default()
    };
    let (logger, guard, memory) = start_memory(config);
    until(|| {
        logger
            .shared
            .counters
            .writer_iterations
            .load(Ordering::Relaxed)
            > 0
            && logger
                .shared
                .counters
                .fallback_iterations
                .load(Ordering::Relaxed)
                > 0
    });
    let start = Instant::now();
    thread::sleep(Duration::from_millis(260));
    let max_iterations = u64::try_from(start.elapsed().as_millis() / 100).unwrap() + 3;
    assert!(
        logger
            .shared
            .counters
            .writer_iterations
            .load(Ordering::Relaxed)
            <= max_iterations
    );
    assert_eq!(
        logger
            .shared
            .counters
            .fallback_iterations
            .load(Ordering::Relaxed),
        1
    );
    assert!(memory.records().is_empty());
    let start = Instant::now();
    assert_eq!(
        logger.fatal("wake.priority", |_| Ok(())),
        Receipt::DiskConfirmed
    );
    assert_eq!(
        guard.finish(Duration::from_millis(200)).state,
        Shutdown::Drained
    );
    assert!(start.elapsed() < Duration::from_millis(250));
}
