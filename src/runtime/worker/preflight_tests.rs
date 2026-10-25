use super::*;
use crate::runtime::{
    preflight::{Identity, WorkerProof},
    test_support::RuntimeFixture,
};

#[tokio::test]
async fn stop_and_termination_win_before_failed_worker_proofs_are_consumed() {
    for terminate in [false, true] {
        let runtime = RuntimeFixture::new();
        let proof = runtime
            .hub
            .with(|core| {
                Ok(WorkerProof {
                    identity: Identity::from(&core.store().agent("agent")?),
                    checked: Err(Error::login_required("delayed preflight failure")),
                })
            })
            .await
            .unwrap();
        runtime
            .hub
            .with(move |core| {
                let mut agent = core.store().agent("agent")?;
                if terminate {
                    agent.task_outcome = super::super::domain::TaskOutcome::Cancelled;
                } else {
                    agent.paused = true;
                }
                agent.control = Some(Control::Stop);
                core.store().save(&agent)
            })
            .await
            .unwrap();
        let stopped = runtime
            .hub
            .with(move |core| {
                Ok(matches!(
                    prepare_execution(core, "agent", proof)?,
                    Preparation::Ready(None)
                ))
            })
            .await
            .unwrap();
        assert!(stopped);
        runtime.finish();
    }
}

#[tokio::test]
async fn changed_execution_identity_requires_a_new_proof_before_saving_start() {
    let runtime = RuntimeFixture::new();
    let proof = runtime
        .hub
        .with(|core| {
            Ok(WorkerProof {
                identity: Identity::from(&core.store().agent("agent")?),
                checked: Err(Error::login_required("stale failure must not be consumed")),
            })
        })
        .await
        .unwrap();
    runtime
        .hub
        .with(|core| {
            let mut agent = core.store().agent("agent")?;
            agent.execution_id = Some("replacement".into());
            core.store().save(&agent)
        })
        .await
        .unwrap();
    assert!(
        runtime
            .hub
            .with(move |core| Ok(matches!(
                prepare_execution(core, "agent", proof)?,
                Preparation::Refresh
            )))
            .await
            .unwrap()
    );
    assert_eq!(
        runtime
            .hub
            .with(|core| Ok(core.store().agent("agent")?.execution_id))
            .await
            .unwrap()
            .as_deref(),
        Some("replacement")
    );
    runtime.finish();
}
