//! Streaming regressions.
#[cfg(feature = "comb")]
use super::*;

#[tokio::test]
#[cfg(feature = "comb")]
async fn comb_identity_uses_committed_reader_and_rejects_old_turns() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let mut agent = core.store().agent("agent")?;
            agent.comb = Some(crate::config::CombBinding {
                credential_file: "/private/comb.json".into(),
                reviewer: false,
            });
            agent.turn_id = Some("comb-turn".into());
            core.store().save(&agent)
        })
        .await
        .unwrap();
    let started = start(&runtime).await;
    let reader = started.reader.downgrade();
    let caller = super::super::super::hub::Caller::Agent {
        id: "agent".into(),
        execution: "execution".into(),
        turn: Some("comb-turn".into()),
    };
    let (entered, gate) = oneshot::channel();
    let (release, released) = WriterGate::new();
    let hub = runtime.hub.clone();
    let held = tokio::spawn(async move {
        hub.with(move |_| {
            entered.send(()).unwrap();
            released.recv().unwrap();
            Ok(())
        })
        .await
    });
    gate.await.unwrap();
    let observed = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        reader.comb(caller.clone()),
    )
    .await;
    release.release();
    held.await.unwrap().unwrap();
    assert_eq!(
        observed.unwrap().unwrap().binding.credential_file,
        std::path::Path::new("/private/comb.json")
    );
    runtime
        .hub
        .with(|core| {
            let mut agent = core.store().agent("agent")?;
            agent.turn_id = Some("replacement".into());
            core.store().save(&agent)
        })
        .await
        .unwrap();
    assert!(matches!(
        reader.comb(caller).await,
        Err(Error::StaleExecution { .. })
    ));
    drop(reader);
    finish(started);
    runtime.finish();
}
