use super::*;
use crate::runtime::test_support::{RuntimeFixture, failure_log};
use std::sync::atomic::AtomicUsize;

#[test]
fn auth_shutdown_observes_all_jobs_without_rescheduling_or_losing_primary() {
    let Some(log) = failure_log(
        module_path!(),
        "auth_shutdown_observes_all_jobs_without_rescheduling_or_losing_primary",
        || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                for primary in [false, true] {
                    check_shutdown_drain(primary).await;
                }
            });
        },
    ) else {
        return;
    };
    assert_eq!(log.matches("auth.job_join_failed").count(), 2);
    assert_eq!(log.matches("shutdown_observe").count(), 4);
    assert!(!log.contains("secret auth request"));
}

async fn check_shutdown_drain(primary: bool) {
    let fixture = RuntimeFixture::new();
    let shutdown = Arc::new(AtomicBool::new(false));
    let mut owner = DeadlineOwner::start(fixture.hub.clone(), fixture.pool(), shutdown.clone())
        .await
        .unwrap();
    let writer_calls = Arc::new(AtomicUsize::new(0));
    let count = writer_calls.clone();
    fixture
        .hub
        .with(move |core| {
            core.store().test_hooks().at_sql("begin", move |_| {
                count.fetch_add(1, Ordering::Relaxed);
                Ok(())
            });
            Ok(())
        })
        .await
        .unwrap();
    owner.jobs.spawn(async {
        (
            Job::Switch("agent".into()),
            Err(Error::provider_timeout("secret auth request")),
        )
    });
    owner.jobs.spawn(async {
        panic!("controlled owner failure");
    });
    let retained = Arc::new(());
    let held = retained.clone();
    let (release, blocked) = tokio::sync::oneshot::channel();
    owner.jobs.spawn(async move {
        blocked.await.unwrap();
        drop(held);
        (Job::Restore("main".into()), Err(Error::agent_busy("busy")))
    });
    tokio::task::yield_now().await;
    let mut drain = Box::pin(owner.drain(if primary {
        Err(Error::state_io("first terminal error"))
    } else {
        Ok(())
    }));
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(drain.as_mut().poll(cx).is_pending()))
            .await
    );
    assert_eq!(Arc::strong_count(&retained), 2);
    if primary {
        assert_eq!(fixture.hub.wake.failure().unwrap().code(), "state_io");
        assert!(shutdown.load(Ordering::Acquire));
    }
    release.send(()).unwrap();
    let result = drain.await.unwrap_err();
    assert_eq!(
        result.code(),
        if primary {
            "state_io"
        } else {
            "internal_error"
        }
    );
    assert_eq!(fixture.hub.wake.failure().unwrap().code(), result.code());
    assert_eq!(Arc::strong_count(&retained), 1);
    assert!(owner.jobs.is_empty());
    assert!(owner.retries.is_empty());
    assert!(owner.restore.is_empty());
    assert_eq!(writer_calls.load(Ordering::Relaxed), 0);
    drop(owner);
    fixture.finish();
}

#[test]
fn restore_failures_retain_existing_retry_and_terminal_decisions() {
    let Some(log) = failure_log(
        module_path!(),
        "restore_failures_retain_existing_retry_and_terminal_decisions",
        || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let fixture = RuntimeFixture::new();
                let mut owner =
                    DeadlineOwner::start(fixture.hub.clone(), fixture.pool(), Arc::default())
                        .await
                        .unwrap();
                let key = Job::Restore("main".into());
                owner.active.insert(key.clone());
                owner
                    .complete(Some(Ok((
                        key.clone(),
                        Err(Error::provider_timeout("secret restore")),
                    ))))
                    .await
                    .unwrap();
                assert!(owner.retries.contains_key(&key));
                assert_eq!(owner.restore.pop_front().as_deref(), Some("main"));
                assert!(!owner.active.contains(&key));
                owner
                    .complete(Some(Ok((
                        key.clone(),
                        Err(Error::authentication_required("secret account")),
                    ))))
                    .await
                    .unwrap();
                assert!(owner.restore.is_empty());
                assert!(owner.retries.is_empty());
                // Ordinary failed jobs during shutdown are observed without becoming fatal.
                owner
                    .jobs
                    .spawn(async move { (key, Err(Error::agent_busy("busy"))) });
                owner.drain(Ok(())).await.unwrap();
                assert!(fixture.hub.wake.failure().is_none());
                drop(owner);
                fixture.finish();
            });
        },
    ) else {
        return;
    };
    let events: Vec<serde_json::Value> = log
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let decisions: Vec<_> = events
        .iter()
        .filter(|e| e["event"] == "auth.restore_failed")
        .map(|e| e["decision"].as_str().unwrap())
        .collect();
    assert_eq!(decisions, ["retry", "terminal", "shutdown_observe"]);
    assert!(!log.contains("secret restore"));
    assert!(!log.contains("secret account"));
}
