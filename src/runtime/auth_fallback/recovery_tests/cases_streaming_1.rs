//! Streaming regressions.
use super::*;

#[tokio::test]
async fn turn_confirmation_invalidates_older_live_reads_even_from_available_unconfirmed() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let mut a = core.store().agent("agent")?;
            core.auth_signal(
                "main",
                a.auth_generation,
                "provider",
                signals::Signal::limited(AuthReason::UsageLimit),
            )?;
            let old = core.store().availability("main", a.auth_generation)?;
            core.store().expire_login_windows(old.retry_at.unwrap())?;
            let before = core.store().availability("main", a.auth_generation)?;
            a.turn_auth_revision = Some(before.revision);
            core.confirm_login_turn(&a)?;
            core.auth_live_read(
                "main",
                a.auth_generation,
                "provider",
                (before.revision, None),
                read(json!(false)),
            )?;
            let after = core.store().availability("main", a.auth_generation)?;
            assert_eq!(after.state, AvailabilityState::Available);
            assert!(after.revision > before.revision);
            assert!(after.retry_at.is_none());
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}
