use super::*;
use crate::{runtime::test_support::RuntimeFixture, state::KeyAuthority};

fn key(revision: u64, bytes: &[u8]) -> KeyAuthority {
    KeyAuthority {
        revision: std::num::NonZeroU64::new(revision).unwrap(),
        fingerprint: crate::config::KeyFingerprint::digest(bytes),
    }
}

#[test]
fn restarted_store_rejects_old_key_confirmation_limit_read_and_restoration() {
    use crate::runtime::test_support::{CoreBuilder, agent};
    let temp = crate::runtime::test_directory();
    let first = key(1, b"a");
    let latest = key(3, b"a");
    {
        let core = CoreBuilder::new(temp.path()).build();
        let mut agent = agent();
        agent.execution_key = Some(first.clone());
        core.store()
            .select_key_authority("main", agent.auth_generation, Some(&latest))
            .unwrap();
        let mut value = core
            .store()
            .availability("main", agent.auth_generation)
            .unwrap();
        value.state = AvailabilityState::Limited;
        value.reason = Some(AuthReason::UsageLimit);
        value.until = Some(crate::state::now() + 3600);
        core.store().save_revised(&mut value).unwrap();
        agent.turn_auth_revision = Some(value.revision);
        core.store().save(&agent).unwrap();
    }
    let mut core = CoreBuilder::new(temp.path()).build();
    let agent = core.store().agent("agent").unwrap();
    let before = core
        .store()
        .availability("main", agent.auth_generation)
        .unwrap();
    core.confirm_login_turn(&agent).unwrap();
    let read = serde_json::from_value(serde_json::json!({"ordinaryUsageAllowed":true,
        "rateLimits":{"primary":{"usedPercent":0},"secondary":{"usedPercent":0}}}))
    .unwrap();
    core.auth_live_read(
        "main",
        agent.auth_generation,
        "old",
        (before.revision, Some(first.clone())),
        read,
    )
    .unwrap();
    core.restore_api_attempt("main", agent.auth_generation, Some(&first))
        .unwrap();
    let after = core
        .store()
        .availability("main", agent.auth_generation)
        .unwrap();
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.key, Some(latest));
    assert_eq!(after.state, AvailabilityState::Limited);
    assert_eq!(after.until, before.until);
    assert!(after.confirmed_at.is_none());
}

#[tokio::test]
async fn old_key_callbacks_cannot_change_new_key_hold_or_release_its_probe() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let mut agent = core.store().agent("agent")?;
            let generation = agent.auth_generation;
            let first = key(1, b"a");
            let second = key(2, b"b");
            let third = key(3, b"a");
            core.store()
                .select_key_authority("main", generation, Some(&first))?;
            agent.execution_key = Some(first.clone());
            for next in [&second, &third] {
                core.store()
                    .select_key_authority("main", generation, Some(next))?;
                let mut value = core.store().availability("main", generation)?;
                value.state = AvailabilityState::Unavailable;
                value.reason = Some(AuthReason::Authentication);
                value.probe = Some(Probe {
                    id: "new-probe".into(),
                    agent: agent.id.clone(),
                    message: "new-message".into(),
                    deadline: 123,
                    error: None,
                });
                core.store().save_revised(&mut value)?;
                // Even a coincident availability revision/probe ID cannot grant old
                // A authority over B, or over the later A selection.
                agent.turn_auth_revision = Some(value.revision);
                agent.auth_probe_id = Some("new-probe".into());
                core.confirm_login_turn(&agent)?;
                core.auth_signal(
                    "main",
                    generation,
                    "old-provider",
                    signals::Signal {
                        key: Some(first.clone()),
                        state: AvailabilityState::Unavailable,
                        ..signals::Signal::limited(AuthReason::SpendCap)
                    },
                )?;
                core.release_auth_probe(&mut agent, Some("new-message"))?;
                let after = core.store().availability("main", generation)?;
                assert_eq!(after.revision, value.revision);
                assert_eq!(after.reason, Some(AuthReason::Authentication));
                assert_eq!(after.probe_id(), Some("new-probe"));
                assert_eq!(after.key.as_ref(), Some(next));
            }
            assert!(
                core.store()
                    .select_key_authority("main", generation, Some(&first))
                    .is_err()
            );
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}
