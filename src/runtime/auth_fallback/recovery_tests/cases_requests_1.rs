//! Requests regressions.
use super::*;

#[tokio::test]
async fn exhausted_admission_requests_group_switch_and_irrelevant_profiles_do_not_wake_waiters() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let mut a = core.store().agent("agent")?;
            a.auth_group = Some("plans".into());
            let until = crate::state::now() + 86400;
            core.auth_signal(
                "main",
                a.auth_generation,
                "provider",
                signals::Signal {
                    until: Some(until),
                    ..signals::Signal::limited(AuthReason::UsageLimit)
                },
            )?;
            let input = core.store().next_message(&a.id)?.unwrap();
            assert!(!core.auth_admit(&mut a, &input)?);
            assert_eq!(a.waiting_reason, Some(WaitingReason::Switch));
            assert_eq!(a.auth_switch_pending.as_ref().unwrap().target, "plans");
            a.waiting_reason = Some(WaitingReason::Quota);
            a.quota_wait_until = Some(until);
            a.auth_checked_revision = core.store().auth_revision()?;
            core.store().save(&a)?;
            core.auth_signal(
                "unrelated",
                uuid::Uuid::new_v4(),
                "provider",
                signals::Signal::limited(AuthReason::UsageLimit),
            )?;
            assert!(
                core.store()
                    .quota_waiters(r#"{"plans":["main","second"]}"#)?
                    .is_empty()
            );
            core.auth_signal(
                "second",
                uuid::Uuid::new_v4(),
                "provider",
                signals::Signal::limited(AuthReason::UsageLimit),
            )?;
            assert_eq!(
                core.store()
                    .quota_waiters(r#"{"plans":["main","second"]}"#)?,
                vec![a.id]
            );
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}
