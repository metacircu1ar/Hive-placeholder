use super::*;
use crate::runtime::test_support::{RuntimeFixture, WriterGate};
use std::sync::atomic::{AtomicBool, Ordering};

fn io_failure() -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_IOERR_WRITE),
        Some("secret SQL and body".into()),
    )
}

#[test]
fn first_owner_failure_wins_over_channel_fallout_and_later_writer_failure() {
    for initial in [
        Error::database_error("writer"),
        Error::state_io("earlier independent control error"),
    ] {
        let wake = Wake::default();
        wake.failed(&initial);
        wake.failed(&closed());
        wake.failed(&Error::database_error("later writer"));
        assert_eq!(wake.failure().unwrap().code(), initial.code());
    }
}

#[tokio::test]
async fn begin_failure_stops_writer_and_never_applies_request() {
    let fixture = RuntimeFixture::new();
    fixture
        .hub
        .with(|c| {
            c.store()
                .test_hooks()
                .at_sql("begin", |_| Err(io_failure()));
            Ok(())
        })
        .await
        .unwrap();
    let executed = Arc::new(AtomicBool::new(false));
    let inside = executed.clone();
    let failure = fixture
        .hub
        .with(move |_| {
            inside.store(true, Ordering::Relaxed);
            Ok(())
        })
        .await
        .unwrap_err();
    assert_eq!(failure.code(), "daemon_stopping");
    assert!(!executed.load(Ordering::Relaxed));
    assert_eq!(
        super::super::store::sqlite_code(&fixture.hub.wake.failure().unwrap()),
        rusqlite::ffi::SQLITE_IOERR_WRITE
    );
    assert_eq!(
        super::super::store::sqlite_code(&fixture.finish_failed()),
        rusqlite::ffi::SQLITE_IOERR_WRITE
    );
}

#[tokio::test]
async fn poisoned_job_stops_queued_retained_job_and_never_checkpoints() {
    let fixture = RuntimeFixture::new();
    let hub = fixture.hub.clone();
    let (entered, started) = oneshot::channel();
    let (release, gate) = WriterGate::new();
    let checkpoint = Arc::new(AtomicBool::new(false));
    let checkpoint_hook = checkpoint.clone();
    let first = tokio::spawn(async move {
        hub.with(move |c| {
            c.store().test_hooks().before_checkpoint(move || {
                checkpoint_hook.store(true, Ordering::Relaxed);
            });
            c.store()
                .test_hooks()
                .at_sql("commit", |db| db.execute_batch("ROLLBACK"));
            // Lose the current nested savepoint now, so its cleanup fails and poisons.
            c.store()
                .test_hooks()
                .sql("commit", c.store().test_connection())
                .unwrap();
            entered.send(()).unwrap();
            gate.recv().unwrap();
            Err::<(), _>(Error::invalid_request("primary request error"))
        })
        .await
    });
    started.await.unwrap();
    let retained = Arc::new(());
    let released = Arc::downgrade(&retained);
    let executed = Arc::new(AtomicBool::new(false));
    let inside = executed.clone();
    let mut second = Box::pin(fixture.hub.with_retained(retained, move |_| {
        inside.store(true, Ordering::Relaxed);
        Ok(())
    }));
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(second.as_mut().poll(cx).is_pending()))
            .await
    );
    release.release();
    assert_eq!(first.await.unwrap().unwrap_err().code(), "invalid_request");
    assert_eq!(second.as_mut().await.unwrap_err().code(), "daemon_stopping");
    drop(second);
    assert!(!executed.load(Ordering::Relaxed));
    assert!(!checkpoint.load(Ordering::Relaxed));
    assert!(released.upgrade().is_none());
    assert_eq!(fixture.finish_failed().code(), "database_error");
}

#[tokio::test]
async fn commit_failure_rejects_success_and_preserves_failed_jobs_primary_error() {
    for fail_job in [false, true] {
        let fixture = RuntimeFixture::new();
        let failure = fixture
            .hub
            .with(move |c| {
                c.store()
                    .test_hooks()
                    .at_sql("commit", |_| Err(io_failure()));
                if fail_job {
                    Err(Error::invalid_request("primary request error"))
                } else {
                    Ok(())
                }
            })
            .await
            .unwrap_err();
        assert_eq!(
            failure.code(),
            if fail_job {
                "invalid_request"
            } else {
                "database_error"
            }
        );
        assert_eq!(
            super::super::store::sqlite_code(&fixture.finish_failed()),
            rusqlite::ffi::SQLITE_IOERR_WRITE
        );
    }
}

#[test]
fn checkpoint_failures_warn_without_retracting_committed_replies_or_stopping_writer() {
    const ISOLATED: &str = "HIVE_CHECKPOINT_LOG_TEST";
    if std::env::var_os(ISOLATED).is_none() {
        let (_, module) = module_path!().split_once("::").unwrap();
        let mut command = crate::test_support::rerun_isolated(
            &format!(
                "{module}::checkpoint_failures_warn_without_retracting_committed_replies_or_stopping_writer"
            ),
            false,
        );
        command.env(ISOLATED, "1");
        crate::test_support::run_isolated(&mut command);
        return;
    }
    let directory = crate::runtime::test_directory();
    let path = directory.path().join("checkpoint.jsonl");
    let (backend, guard) = hive_log::start(&path, hive_log::Config::default()).unwrap();
    backend.install_global().unwrap();
    for (phase, deferred) in [
        ("checkpoint_passive", false),
        ("checkpoint_truncate", false),
        ("checkpoint_passive", true),
    ] {
        let fixture = RuntimeFixture::new();
        fixture
            .hub
            .blocking_with(move |core| {
                let mut agent = core.store().agent("agent")?;
                agent.paused = true;
                core.store().save(&agent)?;
                if deferred {
                    core.store().test_hooks().request_deferred_checkpoint();
                } else {
                    core.store().test_hooks().before_checkpoint(|| {});
                }
                core.store()
                    .test_hooks()
                    .at_sql(phase, |_| Err(io_failure()));
                Ok(())
            })
            .unwrap();
        // This second job runs after the idle or deferred checkpoint failed. The
        // acknowledged mutation remains durable and the writer remains available.
        assert!(
            fixture
                .hub
                .blocking_with(|core| Ok(core.store().agent("agent")?.paused))
                .unwrap()
        );
        assert!(fixture.hub.wake.failure().is_none());
        fixture.finish();
    }
    assert_eq!(
        guard.finish(std::time::Duration::from_secs(1)).state,
        hive_log::Shutdown::Drained
    );
    let log = std::fs::read_to_string(path).unwrap();
    let records: Vec<serde_json::Value> = log
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(
        records
            .iter()
            .filter(|r| r["event"] == "writer.checkpoint_failed"
                && r["sqlite_code"] == rusqlite::ffi::SQLITE_IOERR_WRITE
                && r["decision"] == "defer")
            .count(),
        3
    );
    assert!(!log.contains("secret SQL and body"));
}

#[test]
fn lifecycle_stop_does_not_need_a_closed_or_saturated_writer_queue() {
    for closed in [false, true] {
        let fixture = RuntimeFixture::new();
        let mut hub = fixture.hub.clone();
        let (sender, receiver) = mpsc::channel(1);
        hub.sender = sender;
        let (job, _reply) = Job::new(|_| Ok(()), ());
        assert!(hub.sender.try_send(job).is_ok());
        let receiver = (!closed).then_some(receiver);
        assert_eq!(hub.sender.is_closed(), closed);
        assert_eq!(hub.sender.capacity(), usize::from(closed));
        let error = Error::state_io_from(std::io::Error::from_raw_os_error(libc::EIO));
        hub.stop_after_failure(&error);
        assert!(hub.shutdown.load(Ordering::Acquire));
        assert!(hub.uncertain.load(Ordering::Acquire));
        assert_eq!(hub.wake.failure().unwrap().code(), "state_io");
        hub.stop_after_failure(&Error::database_error("later"));
        assert_eq!(hub.wake.failure().unwrap().code(), "state_io");
        drop(receiver);
        drop(hub);
        fixture.finish();
    }
}
