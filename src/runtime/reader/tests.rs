use super::*;
use crate::runtime::test_support::{RuntimeFixture, WriterGate};
use serde_json::json;

async fn start(runtime: &RuntimeFixture) -> Started {
    let shutdown = runtime
        .hub
        .with(|core| Ok(core.shutdown().clone()))
        .await
        .unwrap();
    Reader::start(
        Queries::open_read_only(&runtime.temp.path().join("runtime.sqlite")).unwrap(),
        runtime.hub.published.clone(),
        shutdown,
        Arc::default(),
        runtime.hub.wake.clone(),
    )
}
async fn read(reader: &Reader, request: Value) -> Result<Value> {
    reader
        .request(
            &crate::runtime::hub::Caller::Human,
            api::RequestEnvelope::new(request),
        )
        .await
        .map(|reply| reply.value)
}
fn finish(started: Started) {
    drop(started.reader);
    started.thread.join().unwrap().unwrap();
}

async fn reply_and_publication_precede_checkpoint(deferred: bool) {
    // Hold the writer inside the reply closure until the caller queues follow-up work.
    struct ReplyGate(std::sync::mpsc::Receiver<()>);
    impl Drop for ReplyGate {
        fn drop(&mut self) {
            let _ = self.0.recv();
        }
    }
    let runtime = RuntimeFixture::new();
    let started = start(&runtime).await;
    let (entered, checkpoint_started) = oneshot::channel();
    let (release, released) = WriterGate::new();
    let (release_reply, replied) = WriterGate::new();
    let hub = runtime.hub.clone();
    let mut write = tokio::spawn(async move {
        hub.with_retained(ReplyGate(replied), move |core| {
            let mut agent = core.store().agent("agent")?;
            agent.paused = true;
            core.store().save(&agent)?;
            core.started("checkpoint-fixture".into());
            let hooks = core.store().test_hooks();
            if deferred {
                // The WAL threshold's checkpoint runs whatever the queue holds.
                hooks.request_deferred_checkpoint();
                let mut gate = Some((entered, released));
                hooks.at_sql("checkpoint_passive", move |_| {
                    if let Some((entered, released)) = gate.take() {
                        entered.send(()).unwrap();
                        released.recv().unwrap();
                    }
                    Ok(())
                });
            } else {
                hooks.before_checkpoint(move || {
                    entered.send(()).unwrap();
                    released.recv().unwrap();
                });
            }
            Ok(())
        })
        .await
    });
    let acknowledged = tokio::time::timeout(std::time::Duration::from_secs(2), &mut write).await;
    let mut followup = Box::pin(runtime.hub.with(|core| {
        core.started("followup-fixture".into());
        Ok(())
    }));
    let queued = std::future::poll_fn(|cx| {
        std::task::Poll::Ready(std::future::Future::poll(followup.as_mut(), cx).is_pending())
    })
    .await;
    release_reply.release();
    let checkpoint_entered =
        tokio::time::timeout(std::time::Duration::from_secs(2), checkpoint_started).await;
    let observed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        let ping = read(&started.reader, json!({"op":"ping"})).await?;
        let status = read(&started.reader, json!({"op":"status", "target":"agent"})).await?;
        Ok::<_, Error>((ping, status))
    })
    .await;
    // Release the checkpoint even when the regression assertion fails.
    release.release();
    if acknowledged.is_err() {
        write.await.unwrap().unwrap();
    }
    followup.await.unwrap();
    runtime.hub.with(|_| Ok(())).await.unwrap();
    finish(started);
    runtime.finish();
    acknowledged
        .expect("durable reply waited for checkpoint")
        .unwrap()
        .unwrap();
    assert!(queued);
    checkpoint_entered.unwrap().unwrap();
    let (ping, status) = observed.unwrap().unwrap();
    assert_eq!(ping["resident_actors"], 1);
    assert_eq!(status["agent"]["paused"], true);
}

async fn output_fixture(runtime: &RuntimeFixture) -> String {
    use crate::runtime::domain::{AgentState, EventKind, TaskOutcome};
    runtime
        .hub
        .with(|core| {
            let mut a = core.store().agent("agent")?;
            a.state = AgentState::Stopped;
            a.task_outcome = TaskOutcome::Completed;
            a.turn_id = None;
            a.reserved_turn = None;
            a.completion_pending = None;
            a.conversation_clean = true;
            core.clear_running();
            core.store().release_claim(&a.id)?;
            core.store().save(&a)?;
            core.store().undeliverable(&a.id)?;
            core.store().event(
                &a,
                EventKind::AssistantMessage,
                json!({"text":"x".repeat(20_000)}),
            )?;
            let events = core.store().events(&a.id, 0, 200)?;
            Ok(
                events["events"].as_array().unwrap().last().unwrap()["data"]["artifact"]["id"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            )
        })
        .await
        .unwrap()
}

fn agent_caller(turn: &str) -> super::super::hub::Caller {
    super::super::hub::Caller::Agent {
        id: "agent".into(),
        execution: "execution".into(),
        turn: Some(turn.into()),
    }
}
fn stalled_request() -> api::RequestEnvelope {
    api::RequestEnvelope::new(json!({"op":"agents","stalled_seconds":1}))
}

async fn seed_stalled_agent(runtime: &RuntimeFixture) {
    runtime
        .hub
        .with(|core| {
            let mut quiet = core.store().agent("agent")?;
            quiet.id = "quiet".into();
            quiet.state = crate::runtime::domain::AgentState::Idle;
            quiet.turn_id = None;
            quiet.execution_id = None;
            core.store().save(&quiet)?;
            core.store().event(
                &quiet,
                crate::runtime::domain::EventKind::Spawned,
                json!({}),
            )
        })
        .await
        .unwrap();
    rusqlite::Connection::open(runtime.temp.path().join("runtime.sqlite"))
        .unwrap()
        .execute("UPDATE events SET at=1 WHERE agent='quiet'", [])
        .unwrap();
}

async fn unlink_purged_artifacts(runtime: &RuntimeFixture) {
    let crate::runtime::store::ArtifactDeletions { dir, ids } = runtime
        .hub
        .with(|c| c.store().pending_artifact_deletions())
        .await
        .unwrap();
    crate::runtime::store::unlink_artifacts(&dir, &ids).unwrap();
    runtime
        .hub
        .with(move |c| c.store().acknowledge_artifact_deletions(&ids))
        .await
        .unwrap();
}

mod cases_behaviour_1;
mod cases_indexing_1;
mod cases_lifecycle_1;
mod cases_recovery_1;
mod cases_services_1;
mod cases_streaming_1;
mod cases_transport_1;
