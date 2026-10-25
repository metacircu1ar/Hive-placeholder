//! Recovery regressions.
use super::*;

#[tokio::test]
async fn inactive_uncertain_probe_expires_without_changing_native_recovery() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let mut a = core.store().agent("agent")?;
            a.auth_group = Some("plans".into());
            core.auth_signal(
                "main",
                a.auth_generation,
                "provider",
                signals::Signal::limited(AuthReason::UsageLimit),
            )?;
            let limited = core.store().availability("main", a.auth_generation)?;
            core.store()
                .expire_login_windows(limited.retry_at.unwrap())?;
            let m = core.store().next_message(&a.id)?.unwrap();
            assert!(core.auth_admit(&mut a, &m)?);
            core.store().message_state(&m, MessageState::Uncertain)?;
            a.state = AgentState::Stopped;
            a.native_input_attempted = true;
            a.recovery_fence = Some(crate::runtime::domain::RecoveryFence::Failure(
                "original fence".into(),
            ));
            core.store().save(&a)?;
            let lease = core.store().availability("main", a.auth_generation)?;
            let deadline = lease.probe_deadline().unwrap();
            core.started(a.id.clone());
            core.reconcile_auth_probes(deadline)?;
            assert_eq!(
                core.store()
                    .availability("main", a.auth_generation)?
                    .probe_id(),
                lease.probe_id()
            );
            core.stopped(&a.id);
            core.reconcile_auth_probes(deadline - 1)?;
            assert_eq!(
                core.store()
                    .availability("main", a.auth_generation)?
                    .probe_id(),
                lease.probe_id()
            );
            core.reconcile_auth_probes(deadline)?;
            let expired = core.store().availability("main", a.auth_generation)?;
            assert!(expired.probe_id().is_none());
            assert!(expired.retry_at.is_some());
            assert_eq!(core.store().agent(&a.id)?.recovery_fence, a.recovery_fence);
            assert_eq!(
                core.store().probe_input(&a.id, &m.id)?,
                Some(MessageState::Uncertain)
            );
            // Invalid missing input holds just this binding and removes its deadline.
            let mut invalid = expired;
            invalid.probe = Some(crate::runtime::auth_fallback::Probe {
                id: a.auth_probe_id.clone().expect("probe identity"),
                agent: a.id.clone(),
                message: "missing".into(),
                deadline: 1,
                error: None,
            });
            core.store().save_availability(&invalid)?;
            assert_eq!(core.reconcile_auth_probes(deadline)?, 1);
            assert_eq!(
                core.store()
                    .availability("main", a.auth_generation)?
                    .probe_error(),
                Some(ProbeError::MissingInput)
            );
            assert_eq!(core.reconcile_auth_probes(deadline)?, 0);
            assert_eq!(core.store().next_deadline()?, None);
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}

#[tokio::test]
async fn repeated_cutoffs_have_bounded_identity_and_keep_original_handoff_history() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let mut a = core.store().agent("agent")?;
            a.auth_group = Some("plans".into());
            let mut previous = "message".to_string();
            for attempt in 1..=15 {
                let m = core.store().next_message(&a.id)?.unwrap();
                assert_eq!(m.id, previous);
                core.store().message_state(&m, MessageState::Failed)?;
                core.auth_cutoff(&mut a, &previous, "turn")?;
                core.auth_cutoff(&mut a, &previous, "turn")?;
                let next = core.store().next_message(&a.id)?.unwrap();
                assert!(next.id.len() < 100);
                assert!(next.body.contains("message message,"));
                assert!(next.body.contains(&format!("attempt {attempt}")));
                previous = next.id;
            }
            assert_eq!(
                core.store().messages(&a.id, None, 100)?["messages"]
                    .as_array()
                    .unwrap()
                    .len(),
                16
            );
            let handoff = core.store().auth_handoff(&a.id, AuthReason::UsageLimit)?;
            assert!(handoff.contains("corpus input"));
            assert!(!handoff.contains("continuation attempt"));
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}

#[tokio::test]
async fn startup_cutoff_recovery_matches_delivery_and_is_idempotent() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let mut a = core.store().agent("agent")?;
            a.auth_group = Some("plans".into());
            let input = core.store().next_message(&a.id)?.unwrap();
            core.store().turn_started(
                &a,
                crate::runtime::api::events::TurnStarted {
                    attachments: &[],
                    turn_id: "turn",
                    message_id: &input.id,
                    sender: "human",
                    selection: crate::runtime::api::events::TurnSelection {
                        model: &a.model,
                        effort: &a.effort,
                        options_revision: a.options_revision,
                        selection_verification: "requested_unverified",
                    },
                },
            )?;
            core.store()
                .message_state(&input, MessageState::Uncertain)?;
            a.auth_cutoff = Some(crate::runtime::domain::AuthCutoff::Bound {
                turn: "turn".into(),
                message: input.id.clone(),
            });
            a.state = AgentState::Stopped;
            a.recovery_fence = Some(crate::runtime::domain::RecoveryFence::Failure(
                "keep fence".into(),
            ));
            core.store().save(&a)?;
            core.recover_auth_cutoffs()?;
            core.recover_auth_cutoffs()?;
            let recovered = core.store().agent(&a.id)?;
            assert!(recovered.auth_cutoff.is_none());
            assert!(
                recovered
                    .auth_cutoff
                    .as_ref()
                    .and_then(crate::runtime::domain::AuthCutoff::message)
                    .is_none()
            );
            assert_eq!(recovered.recovery_fence, a.recovery_fence);
            assert_eq!(
                core.store().probe_input(&a.id, &input.id)?,
                Some(MessageState::Uncertain)
            );
            assert_eq!(
                core.store().messages(&a.id, None, 100)?["messages"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}

#[tokio::test]
async fn verified_new_generation_is_published_once_and_wakes_old_generation_waiters() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let mut a = core.store().agent("agent")?;
            a.auth_group = Some("plans".into());
            core.auth_signal(
                "main",
                a.auth_generation,
                "provider",
                signals::Signal {
                    state: AvailabilityState::Unavailable,
                    ..signals::Signal::limited(AuthReason::SpendCap)
                },
            )?;
            a.waiting_reason = Some(WaitingReason::Quota);
            a.auth_checked_revision = core.store().auth_revision()?;
            core.store().save(&a)?;
            let repaired = uuid::Uuid::new_v4();
            core.restore_verified_auth("main", repaired)?;
            let published = core.store().availability("main", repaired)?;
            assert!(published.revision > a.auth_checked_revision);
            assert_eq!(published.state, AvailabilityState::Available);
            assert!(published.confirmed_at.is_none());
            assert_eq!(
                core.store().quota_waiters(r#"{"plans":["main"]}"#)?,
                vec![a.id]
            );
            core.restore_verified_auth("main", repaired)?;
            assert_eq!(
                core.store().availability("main", repaired)?.revision,
                published.revision
            );
            assert_eq!(
                core.store().availability("main", a.auth_generation)?.state,
                AvailabilityState::Unavailable
            );
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}
