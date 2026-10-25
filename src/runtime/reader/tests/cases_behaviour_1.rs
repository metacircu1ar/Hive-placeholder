//! Behaviour regressions.
use super::*;

#[tokio::test]
async fn reads_committed_state_while_writer_is_blocked_and_sees_acknowledged_changes() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let mut agent = core.store().agent("agent")?;
            agent.state = crate::runtime::domain::AgentState::Idle;
            core.store().save(&agent)
        })
        .await
        .unwrap();
    let started = start(&runtime).await;
    let initial_ping = read(&started.reader, json!({"op":"ping"})).await.unwrap();
    assert_eq!(initial_ping["storage"]["ready"], true);
    let (entered, wait) = oneshot::channel();
    let (release, released) = WriterGate::new();
    let hub = runtime.hub.clone();
    let write = tokio::spawn(async move {
        hub.with(move |core| {
            let mut agent = core.store().agent("agent")?;
            agent.paused = true;
            core.store().save(&agent)?;
            core.started("publication-fixture".into());
            entered.send(()).unwrap();
            released.recv().unwrap();
            Ok(())
        })
        .await
    });
    wait.await.unwrap();
    let prior = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        let ping = read(&started.reader, json!({"op":"ping"})).await?;
        let status = read(&started.reader, json!({"op":"status", "target":"agent"})).await?;
        Ok::<_, Error>((ping, status))
    })
    .await;
    // Always unblock the writer before assertions so a failure cannot strand it.
    release.release();
    write.await.unwrap().unwrap();
    let (prior_ping, prior) = prior.unwrap().unwrap();
    assert_eq!(
        prior_ping["writer_observation"]["sequence"],
        initial_ping["writer_observation"]["sequence"]
    );
    assert_eq!(
        prior_ping["resident_actors"],
        initial_ping["resident_actors"]
    );
    let committed_ping = read(&started.reader, json!({"op":"ping"})).await.unwrap();
    assert_eq!(committed_ping["resident_actors"], 1);
    assert_eq!(prior_ping["storage"]["ready"], true);
    assert_eq!(committed_ping["storage"]["ready"], false);
    assert!(
        committed_ping["writer_observation"]["sequence"]
            .as_u64()
            .unwrap()
            > prior_ping["writer_observation"]["sequence"]
                .as_u64()
                .unwrap()
    );
    assert_eq!(
        prior_ping["writer_observation"]["runtime_id"],
        committed_ping["writer_observation"]["runtime_id"]
    );
    assert_eq!(prior["agent"]["paused"], false);
    assert_eq!(
        read(&started.reader, json!({"op":"status", "target":"agent"}))
            .await
            .unwrap()["agent"]["paused"],
        true
    );
    finish(started);
    runtime.finish();
}

#[tokio::test]
async fn shutdown_rejects_reads_and_joins_reader_before_writer() {
    let runtime = RuntimeFixture::new();
    let started = start(&runtime).await;
    runtime
        .hub
        .with(|core| {
            core.shutdown().store(true, Ordering::Relaxed);
            Ok(())
        })
        .await
        .unwrap();
    crate::test_support::assert_code!(
        read(&started.reader, json!({"op":"ping"})).await,
        "daemon_stopping"
    );
    finish(started);
    runtime.finish();
}

#[tokio::test]
async fn reader_panic_signals_shutdown_without_cancelling_inflight_writer() {
    let runtime = RuntimeFixture::new();
    let shutdown = runtime
        .hub
        .with(|core| Ok(core.shutdown().clone()))
        .await
        .unwrap();
    let uncertain = Arc::new(AtomicBool::new(false));
    let notifier = Arc::<Wake>::default();
    let started = Reader::start(
        Queries::open_read_only(&runtime.temp.path().join("runtime.sqlite")).unwrap(),
        runtime.hub.published.clone(),
        shutdown.clone(),
        uncertain.clone(),
        notifier.clone(),
    );
    let (entered, wait) = oneshot::channel();
    let (release, released) = WriterGate::new();
    let hub = runtime.hub.clone();
    let write = tokio::spawn(async move {
        hub.with(move |core| {
            entered.send(()).unwrap();
            released.recv().unwrap();
            let mut agent = core.store().agent("agent")?;
            agent.paused = true;
            core.store().save(&agent)
        })
        .await
    });
    wait.await.unwrap();
    let wake = notifier.supervisor.notified();
    tokio::pin!(wake);
    wake.as_mut().enable();
    let (reply, failed) = oneshot::channel();
    started
        .reader
        .sender
        .send(Job::Control(Box::new(ControlJob {
            test_hooks: super::super::super::test_support::TestHooks::before_job(|| {
                panic!("injected reader owner panic");
            }),
            args: api::RequestEnvelope::new(json!({"op":"ping"})),
            owner: None,
            reply,
        })))
        .await
        .unwrap();
    let failure = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        let closed = failed.await.is_err();
        wake.await;
        (
            closed,
            shutdown.load(Ordering::Relaxed),
            uncertain.load(Ordering::Relaxed),
        )
    })
    .await;
    release.release();
    write.await.unwrap().unwrap();
    assert_eq!(failure.unwrap(), (true, true, true));
    assert!(
        runtime
            .hub
            .with(|core| Ok(core.store().agent("agent")?.paused))
            .await
            .unwrap()
    );
    drop(started.reader);
    assert!(started.thread.join().is_err());
    runtime.finish();
}

#[tokio::test]
async fn join_settlement_is_included_in_published_readiness() {
    let runtime = RuntimeFixture::new();
    let started = start(&runtime).await;
    runtime
        .hub
        .with(|core| {
            let s = core.store();
            let mut requester = s.agent("agent")?;
            requester.state = crate::runtime::domain::AgentState::Stopped;
            requester.turn_id = None;
            requester.control = None;
            requester.paused = false;
            s.save(&requester)?;
            let mut worker = requester.clone();
            worker.id = "worker".into();
            s.save(&worker)?;
            let id = s.create_join("agent", "published", None)?;
            s.enqueue("member", "agent", "worker", "work")?;
            s.join_member(&id, "agent", "member", 0)?;
            s.undeliverable("worker")?;
            Ok(())
        })
        .await
        .unwrap();
    let ping = read(&started.reader, json!({"op":"ping"})).await.unwrap();
    assert_eq!(ping["storage"]["ready"], true);
    let messages = read(&started.reader, json!({"op":"messages","target":"agent"}))
        .await
        .unwrap();
    assert!(
        messages["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["kind"] == "join_result")
    );
    finish(started);
    runtime.finish();
}

#[tokio::test]
async fn failed_batch_does_not_publish_its_readiness_or_actor_changes() {
    let runtime = RuntimeFixture::new();
    let sequence = runtime.hub.published.observation.lock_or_panic().sequence;
    let result = runtime
        .hub
        .with(|core| {
            core.started("uncommitted".into());
            core.store().test_hooks().at_sql("commit", |_| {
                Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_IOERR_WRITE),
                    None,
                ))
            });
            Ok(())
        })
        .await;
    assert_eq!(result.unwrap_err().code(), "database_error");
    assert_eq!(
        runtime.hub.published.observation.lock_or_panic().sequence,
        sequence
    );
    assert_eq!(
        runtime
            .hub
            .published
            .observation
            .lock_or_panic()
            .fields
            .actors,
        0
    );
    assert_eq!(runtime.finish_failed().code(), "database_error");
}
