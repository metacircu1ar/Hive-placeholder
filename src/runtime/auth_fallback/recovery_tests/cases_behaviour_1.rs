//! Behaviour regressions.
use super::*;

#[test]
fn resets_are_normalized_once_and_sparse_markers_do_not_resurrect_limits() {
    let now = 100_000;
    for bad in [
        json!(now + 33 * 86400),
        json!("sensitive raw text"),
        json!(1.5),
    ] {
        let value = limits(
            json!({"primary":{"usedPercent":100,
                "resetsAt":bad},
                "secondary":{"usedPercent":100,
                "resetsAt":now+7200},
                "rateLimitReachedType":"rate_limit_reached"}),
            now,
        );
        let signal = value.signal_at(now).unwrap();
        assert_eq!(signal.until, None);
        assert_eq!(signal.reset_floor, Some(now + 7200));
        assert_eq!(signal.rejected_resets.len(), 1);
        assert!(!format!("{:?}", signal.rejected_resets).contains("sensitive"));
    }
    let mut value = limits(
        json!({"primary":{"usedPercent":100,
            "resetsAt":now+10},
            "rateLimitReachedType":"rate_limit_reached"}),
        now,
    );
    assert!(value.signal_at(now + 40 * 86400).is_none());
    value.merge_at(
        serde_json::from_value(json!({"secondary":{"usedPercent":5},"rateLimitReachedType":null}))
            .unwrap(),
        now + 40 * 86400,
    );
    assert!(value.signal_at(now + 40 * 86400).is_none());
    for past in [json!(0), json!(-100), json!(now - 1)] {
        assert!(
            limits(
                json!({"primary":{"usedPercent":100,
            "resetsAt":past},
            "rateLimitReachedType":"rate_limit_reached"}),
                now
            )
            .signal_at(now)
            .is_none()
        );
    }
}

#[tokio::test]
async fn unknown_deadlines_are_stable_and_mixed_causes_keep_the_longer_floor() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(assert_stable_and_mixed_deadlines)
        .await
        .unwrap();
    runtime.finish();
}

#[tokio::test]
async fn known_expiry_keeps_owner_and_clean_same_binding_unparks_without_discovery() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let mut a = core.store().agent("agent")?;
            a.auth_group = Some("plans".into());
            let mut row = Availability::available("main", a.auth_generation);
            row.state = AvailabilityState::Limited;
            row.until = Some(1);
            row.reason = Some(AuthReason::UsageLimit);
            row.probe = Some(crate::runtime::auth_fallback::Probe {
                id: "exact".into(),
                agent: a.id.clone(),
                message: "message".into(),
                deadline: crate::state::now() + 3600,
                error: None,
            });
            core.store().save_availability(&row)?;
            a.auth_probe_id = row.probe_id().map(str::to_owned);
            a.conversation_clean = true;
            a.state = AgentState::Stopped;
            core.request_auth_boundary(&mut a, "plans".into(), AuthReason::UsageLimit)?;
            a.waiting_reason = Some(WaitingReason::Quota);
            core.store().save(&a)?;
            core.store().expire_login_windows(2)?;
            let available = core.store().availability("main", a.auth_generation)?;
            assert_eq!(available.probe_id(), row.probe_id());
            assert_eq!(available.retry_at, None);
            assert!(core.unpark_auth_waiter(&mut a)?);
            assert!(a.waiting_reason.is_none());
            assert!(a.auth_switch_pending.is_none());
            assert_eq!(a.thread_id.as_deref(), Some("thread"));
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}

#[tokio::test]
async fn diagnostic_hold_wakes_fallback_but_identity_verification_does_not_clear_evidence() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let mut owner = core.store().agent("agent")?;
            owner.auth_group = Some("plans".into());
            owner.auth_probe_id = Some("broken".into());
            core.store().save(&owner)?;
            core.store()
                .ensure_availability("main", owner.auth_generation)?;
            let mut value = core.store().availability("main", owner.auth_generation)?;
            value.retry_at = Some(1);
            value.probe = Some(crate::runtime::auth_fallback::Probe {
                id: owner.auth_probe_id.clone().expect("probe identity"),
                agent: owner.id.clone(),
                message: "missing".into(),
                deadline: crate::state::now() + 3600,
                error: None,
            });
            core.store().save_availability(&value)?;
            let mut peer = owner.clone();
            peer.id = "peer".into();
            peer.auth_probe_id = None;
            core.store().save(&peer)?;
            core.store()
                .enqueue("peer-input", "human", &peer.id, "work")?;
            let input = core.store().next_message(&peer.id)?.unwrap();
            assert!(!core.auth_admit(&mut peer, &input)?);
            assert_eq!(peer.waiting_reason, Some(WaitingReason::Quota));
            core.reconcile_auth_probes(crate::state::now())?;
            let held = core.store().availability("main", owner.auth_generation)?;
            assert!(held.revision > value.revision);
            assert_eq!(held.probe_error(), Some(ProbeError::MissingInput));
            assert!(
                core.store()
                    .quota_waiters(r#"{"plans":["main","second"]}"#)?
                    .contains(&peer.id)
            );
            assert!(core.store().affected_auth_agents("[]")?.contains(&owner.id));
            assert!(!core.auth_admit(&mut peer, &input)?);
            assert_eq!(peer.waiting_reason, Some(WaitingReason::Switch));
            for _ in 0..2 {
                core.restore_verified_auth("main", owner.auth_generation)?;
            }
            let retained = core.store().availability("main", owner.auth_generation)?;
            assert_eq!(retained.revision, held.revision);
            assert_eq!(retained.probe_error(), held.probe_error());
            // The deliberate pinned override bypasses only admission policy.
            peer.auth_group = None;
            assert!(core.auth_admit(&mut peer, &input)?);
            assert_eq!(
                core.store()
                    .availability("main", owner.auth_generation)?
                    .probe_id(),
                held.probe_id()
            );
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}
