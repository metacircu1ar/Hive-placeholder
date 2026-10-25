//! Authentication regressions.
use super::*;

#[tokio::test]
async fn authoritative_read_needs_permission_and_revision_and_does_not_rearm_a_probe() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let a = core.store().agent("agent")?;
            core.auth_signal(
                "main",
                a.auth_generation,
                "provider",
                signals::Signal::limited(AuthReason::UsageLimit),
            )?;
            let initial = core.store().availability("main", a.auth_generation)?;
            core.auth_live_read(
                "main",
                a.auth_generation,
                "provider",
                (initial.revision, None),
                read(json!(null)),
            )?;
            assert_eq!(
                core.store().availability("main", a.auth_generation)?.state,
                AvailabilityState::Limited
            );
            core.auth_live_read(
                "main",
                a.auth_generation,
                "provider",
                (initial.revision - 1, None),
                read(json!(true)),
            )?;
            assert_eq!(
                core.store().availability("main", a.auth_generation)?.state,
                AvailabilityState::Limited
            );
            core.store()
                .expire_login_windows(initial.retry_at.unwrap())?;
            let permitted = core.store().availability("main", a.auth_generation)?;
            for permission in [json!(null), json!(false)] {
                core.auth_live_read(
                    "main",
                    a.auth_generation,
                    "provider",
                    (permitted.revision, None),
                    read(permission),
                )?;
                let still = core.store().availability("main", a.auth_generation)?;
                assert_eq!(still.state, AvailabilityState::Available);
                assert_eq!(still.retry_at, permitted.retry_at);
            }
            let current = core.store().availability("main", a.auth_generation)?;
            core.auth_live_read(
                "main",
                a.auth_generation,
                "provider",
                (current.revision, None),
                read(json!(true)),
            )?;
            let confirmed = core.store().availability("main", a.auth_generation)?;
            assert_eq!(confirmed.retry_at, None);
            assert!(confirmed.confirmed_at.is_some());
            core.auth_live_read(
                "main",
                a.auth_generation,
                "provider",
                (confirmed.revision, None),
                read(json!(false)),
            )?;
            assert_eq!(
                core.store().availability("main", a.auth_generation)?.state,
                AvailabilityState::Limited
            );
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}

#[tokio::test]
async fn probe_admission_is_single_owner_and_transfers_only_unattempted_priority_input() {
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
            let message = core.store().next_message(&a.id)?.unwrap();
            assert!(core.auth_admit(&mut a, &message)?);
            let original_lease = a.auth_probe_id.clone();
            let mut b = a.clone();
            b.id = "peer".into();
            b.auth_probe_id = None;
            core.store().save(&b)?;
            core.store().enqueue("peer-input", "human", &b.id, "work")?;
            let peer = core.store().next_message(&b.id)?.unwrap();
            assert!(!core.auth_admit(&mut b, &peer)?);
            core.store()
                .enqueue_redirect("priority", "human", &mut a, "priority")?;
            let priority = core.store().next_message(&a.id)?.unwrap();
            assert_eq!(priority.id, "priority");
            assert!(core.auth_admit(&mut a, &priority)?);
            assert_eq!(a.auth_probe_id, original_lease);
            core.store()
                .message_state(&priority, MessageState::Uncertain)?;
            assert!(!core.auth_admit(&mut a, &message)?);
            // Re-limiting retains the unresolved lease; expiry cannot admit the peer.
            core.auth_signal(
                "main",
                a.auth_generation,
                "provider",
                signals::Signal {
                    turn_error: true,
                    ..signals::Signal::limited(AuthReason::UsageLimit)
                },
            )?;
            let next = core.store().availability("main", a.auth_generation)?;
            assert_eq!(next.probe_id(), original_lease.as_deref());
            core.store().expire_login_windows(next.retry_at.unwrap())?;
            assert!(!core.auth_admit(&mut b, &peer)?);
            // A stale callback cannot clear a replacement lease on the same binding.
            let mut replaced = core.store().availability("main", a.auth_generation)?;
            replaced.probe.as_mut().unwrap().id = "replacement".into();
            core.store().save_availability(&replaced)?;
            core.release_auth_probe(&mut a, Some("priority"))?;
            assert_eq!(
                core.store()
                    .availability("main", a.auth_generation)?
                    .probe_id(),
                replaced.probe_id()
            );
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}

#[tokio::test]
async fn relimit_preserves_unknown_gate_and_sparse_telemetry_preserves_auth_cause() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let a = core.store().agent("agent")?;
            core.auth_signal(
                "main",
                a.auth_generation,
                "first",
                signals::Signal::limited(AuthReason::UsageLimit),
            )?;
            let old = core.store().availability("main", a.auth_generation)?;
            core.store().expire_login_windows(old.retry_at.unwrap())?;
            let known = crate::state::now() + 7200;
            core.auth_signal(
                "main",
                a.auth_generation,
                "second",
                signals::Signal {
                    until: Some(known),
                    window: Some("secondary".into()),
                    ..signals::Signal::limited(AuthReason::UsageLimit)
                },
            )?;
            let held = core.store().availability("main", a.auth_generation)?;
            assert_eq!(held.until, None);
            assert_eq!(held.retry_at, Some(known));
            core.store().expire_login_windows(known)?;
            assert!(
                core.store()
                    .availability("main", a.auth_generation)?
                    .retry_at
                    .is_some()
            );
            let auth = uuid::Uuid::new_v4();
            core.auth_signal(
                "main",
                auth,
                "provider",
                signals::Signal::limited(AuthReason::Authentication),
            )?;
            let old = core.store().availability("main", auth)?;
            core.store().expire_login_windows(old.retry_at.unwrap())?;
            let permitted = core.store().availability("main", auth)?;
            core.auth_live_read(
                "main",
                auth,
                "provider",
                (permitted.revision, None),
                read(json!(false)),
            )?;
            let after = core.store().availability("main", auth)?;
            assert_eq!(after.revision, permitted.revision);
            assert_eq!(after.reason, permitted.reason);
            core.restore_verified_auth("main", auth)?;
            assert!(core.store().availability("main", auth)?.retry_at.is_none());
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}

#[tokio::test]
async fn auth_verification_retains_usage_and_spend_and_healthy_verification_is_idempotent() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let a = core.store().agent("agent")?;
            for reversed in [false, true] {
                let generation = uuid::Uuid::new_v4();
                let auth = signals::Signal {
                    state: AvailabilityState::Unavailable,
                    ..signals::Signal::limited(AuthReason::Authentication)
                };
                let usage = signals::Signal {
                    until: Some(crate::state::now() + 7200),
                    ..signals::Signal::limited(AuthReason::UsageLimit)
                };
                let pair = if reversed {
                    [auth, usage]
                } else {
                    [usage, auth]
                };
                for signal in pair {
                    core.auth_signal("main", generation, "provider", signal)?;
                }
                let before = core.store().availability("main", generation)?;
                assert_eq!(before.state, AvailabilityState::Unavailable);
                assert_eq!(before.reason, Some(AuthReason::UsageAndAuthentication));
                core.restore_verified_auth("main", generation)?;
                let after = core.store().availability("main", generation)?;
                assert_eq!(after.state, AvailabilityState::Limited);
                assert_eq!(after.retry_at, before.retry_at);
                assert_eq!(after.reason, before.reason);
                core.restore_verified_auth("main", generation)?;
                assert_eq!(
                    core.store().availability("main", generation)?.revision,
                    after.revision
                );
            }
            core.store()
                .ensure_availability("main", a.auth_generation)?;
            let before = core.store().availability("main", a.auth_generation)?;
            core.restore_verified_auth("main", a.auth_generation)?;
            assert_eq!(
                core.store()
                    .availability("main", a.auth_generation)?
                    .revision,
                before.revision
            );
            core.auth_signal(
                "main",
                a.auth_generation,
                "provider",
                signals::Signal {
                    state: AvailabilityState::Unavailable,
                    ..signals::Signal::limited(AuthReason::SpendCap)
                },
            )?;
            let before = core.store().availability("main", a.auth_generation)?;
            core.restore_verified_auth("main", a.auth_generation)?;
            assert_eq!(
                core.store()
                    .availability("main", a.auth_generation)?
                    .revision,
                before.revision
            );
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}

#[tokio::test]
async fn group_attachment_does_not_change_conversation_or_another_agents_probe() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let a = core.store().agent("agent")?;
            core.config_mut()
                .auth_profiles
                .insert("main".into(), a.profile.clone());
            core.config_mut().auth_groups.insert(
                "plans".into(),
                crate::config::AuthGroup::ordered(vec!["main".into()]),
            );
            let mut prepared = prepare::Input::capture(core, "plans".into()).resolve()?;
            for member in &mut prepared.members {
                member.catalog = Some(Ok(vec![crate::runtime::provider::protocol::Model::parse(
                    &json!({"model":a.model,
                "supportedReasoningEfforts":[{"reasoningEffort":a.effort}]}),
                )]));
            }
            let mut value = Availability::available("main", a.auth_generation);
            value.retry_at = Some(1);
            value.reason = Some(AuthReason::UsageLimit);
            value.probe = Some(crate::runtime::auth_fallback::Probe {
                id: "peer-lease".into(),
                agent: "peer".into(),
                message: "peer-message".into(),
                deadline: crate::state::now() + 3600,
                error: None,
            });
            core.store().save_availability(&value)?;
            let mut facts = crate::runtime::core::preflight::Facts::default();
            facts.auth = Some(prepared);
            let args = serde_json::from_value(json!({"target":a.id,"auth":"plans"})).unwrap();
            core.set_auth(&args, &facts)?;
            let after = core.store().agent(&a.id)?;
            assert_eq!(after.auth_group.as_deref(), Some("plans"));
            assert_eq!(
                after.auth_decision,
                Some(super::super::AuthDecision {
                    mode: crate::config::AuthMode::Ordered,
                    reason: super::super::AuthChoice::Sticky
                })
            );
            assert_eq!(after.thread_id, a.thread_id);
            assert_eq!(after.execution_id, a.execution_id);
            assert!(after.auth_switch_pending.is_none());
            assert_eq!(
                core.store()
                    .availability("main", a.auth_generation)?
                    .probe_id(),
                value.probe_id()
            );
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}

#[tokio::test]
async fn paused_probe_releases_only_quota_lease_and_purged_owner_has_valid_reconciliation() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let mut a = core.store().agent("agent")?;
            a.auth_group = Some("plans".into());
            a.paused = true;
            core.auth_signal(
                "main",
                a.auth_generation,
                "provider",
                signals::Signal::limited(AuthReason::UsageLimit),
            )?;
            let limited = core.store().availability("main", a.auth_generation)?;
            core.store()
                .expire_login_windows(limited.retry_at.unwrap())?;
            let input = core.store().next_message(&a.id)?.unwrap();
            assert!(core.auth_admit(&mut a, &input)?);
            core.store()
                .message_state(&input, MessageState::Uncertain)?;
            let lease = core.store().availability("main", a.auth_generation)?;
            core.reconcile_auth_probes(lease.probe_deadline().unwrap())?;
            assert!(core.store().agent(&a.id)?.paused);
            assert_eq!(
                core.store().probe_input(&a.id, &input.id)?,
                Some(MessageState::Uncertain)
            );
            core.store().save_availability(&lease)?;
            a.state = AgentState::Stopped;
            a.task_outcome = crate::runtime::domain::TaskOutcome::Cancelled;
            a.turn_id = None;
            a.conversation_clean = true;
            a.ended_at = Some(crate::state::now());
            core.store().save(&a)?;
            core.store().purge(&a.id, false, false, core.running())?;
            core.reconcile_auth_probes(0)?;
            assert!(
                core.store()
                    .availability("main", a.auth_generation)?
                    .probe_id()
                    .is_none()
            );
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}
