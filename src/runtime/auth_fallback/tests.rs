use super::*;
use crate::runtime::test_support::RuntimeFixture;
use serde_json::json;

fn limited(until: Option<u64>) -> signals::Signal {
    signals::Signal {
        state: AvailabilityState::Limited,
        until,
        window: Some("primary".into()),
        ..signals::Signal::limited(AuthReason::UsageLimit)
    }
}

#[tokio::test]
async fn binding_scoped_limits_and_revision_fenced_confirmation() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let mut agent = core.store().agent("agent")?;
            let old = agent.auth_generation;
            let repaired = uuid::Uuid::new_v4();
            core.store().ensure_availability("main", old)?;
            let before = core.store().availability("main", old)?.revision;
            core.store().ensure_availability("main", repaired)?;
            core.auth_signal("main", repaired, "provider", limited(None))?;
            let newer = core.store().availability("main", repaired)?;
            core.store().ensure_availability("main", old)?;
            core.auth_signal("main", old, "old-provider", limited(Some(123)))?;
            assert_eq!(
                core.store().availability("main", repaired)?.revision,
                newer.revision
            );
            agent.turn_auth_revision = Some(before);
            core.confirm_login_turn(&agent)?;
            assert_eq!(
                core.store().availability("main", old)?.state,
                AvailabilityState::Limited
            );
            core.store().expire_login_windows(crate::state::now())?;
            assert_eq!(
                core.store().availability("main", old)?.state,
                AvailabilityState::Available
            );
            assert_eq!(
                core.store().availability("main", repaired)?.state,
                AvailabilityState::Limited
            );
            agent.auth_generation = repaired;
            agent.turn_auth_revision = Some(newer.revision);
            core.confirm_login_turn(&agent)?;
            let confirmed = core.store().availability("main", repaired)?;
            assert_eq!(confirmed.state, AvailabilityState::Available);
            assert!(confirmed.revision > newer.revision);
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}

#[test]
fn typed_limits_use_latest_window_and_unknown_resets_stay_unknown() {
    let mut limits: signals::CodexLimits =
        serde_json::from_value(json!({"primary":{"usedPercent":100,"resetsAt":10}})).unwrap();
    limits.merge(serde_json::from_value(json!({"secondary":{"usedPercent":100,"resetsAt":20},"rateLimitReachedType":"rate_limit_reached"})).unwrap());
    assert_eq!(limits.signal_at(0).unwrap().until, Some(20));
    limits.merge(
        serde_json::from_value(
            json!({"secondary":{"usedPercent":100},"rateLimitReachedType":"rate_limit_reached"}),
        )
        .unwrap(),
    );
    assert_eq!(limits.signal_at(0).unwrap().until, None);
    limits.merge(serde_json::from_value(json!({"rateLimitReachedType":"unknown"})).unwrap());
    assert!(limits.signal().is_none());
    assert!(
        signals::codex_error(&json!({"codexErrorInfo":"rateLimitExceeded","message":"quota"}))
            .is_none()
    );
    let unknown: signals::ClaudeLimits =
        serde_json::from_value(json!({"status":"future_status","resetsAt":10})).unwrap();
    assert!(unknown.signal().is_none());
}

#[tokio::test]
async fn cutoff_continuation_is_idempotent_across_wait_decisions() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let mut agent = core.store().agent("agent")?;
            agent.auth_group = Some("plans".into());
            core.auth_cutoff(&mut agent, "message", "turn")?;
            core.request_auth_boundary(&mut agent, "different-choice".into(), AuthReason::Manual)?;
            core.auth_cutoff(&mut agent, "message", "turn")?;
            let messages = core.store().messages("agent", None, 100)?;
            assert_eq!(
                messages["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|m| m["kind"] == "continuation")
                    .count(),
                1
            );
            assert_eq!(
                agent.auth_switch_pending.unwrap().target,
                "different-choice"
            );
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}

#[tokio::test]
async fn weaker_or_unknown_window_updates_cannot_clear_unresolved_evidence() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let a = core.store().agent("agent")?;
            core.auth_signal(
                "main",
                a.auth_generation,
                "one",
                signals::Signal {
                    state: AvailabilityState::Unavailable,
                    until: None,
                    window: None,
                    ..signals::Signal::limited(AuthReason::Authentication)
                },
            )?;
            core.auth_signal("main", a.auth_generation, "two", limited(Some(1)))?;
            core.store().expire_login_windows(crate::state::now())?;
            assert_eq!(
                core.store().availability("main", a.auth_generation)?.state,
                AvailabilityState::Unavailable
            );
            let other = uuid::Uuid::new_v4();
            core.auth_signal("main", other, "one", limited(None))?;
            let mut secondary = limited(Some(1));
            secondary.window = Some("secondary".into());
            core.auth_signal("main", other, "two", secondary)?;
            core.store().expire_login_windows(crate::state::now())?;
            let held = core.store().availability("main", other)?;
            assert_eq!(held.state, AvailabilityState::Limited);
            assert!(held.until.is_none());
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}

#[tokio::test]
async fn stale_error_cannot_attach_to_an_unacknowledged_different_input() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let mut a = core.store().agent("agent")?;
            a.auth_group = Some("plans".into());
            a.turn_id = None;
            a.auth_cutoff = Some(crate::runtime::domain::AuthCutoff::Observed {
                turn: "previous-completed-turn".into(),
            });
            a.reserved_turn = Some(crate::runtime::domain::ReservedTurn {
                message_id: "message".into(),
                selection: a.desired_selection(),
            });
            core.auth_uncertain_cutoff(&mut a)?;
            let messages = core.store().messages("agent", None, 100)?;
            assert_eq!(messages["messages"].as_array().unwrap().len(), 1);
            assert!(a.auth_switch_pending.is_none());
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}

#[test]
fn sparse_snapshot_retains_reached_metadata_and_extends_exhausted_window() {
    let mut limits: signals::CodexLimits=serde_json::from_value(json!({"primary":{"usedPercent":100,"resetsAt":10},"rateLimitReachedType":"rate_limit_reached"})).unwrap();
    limits.merge(
        serde_json::from_value(json!({"secondary":{"usedPercent":100,"resetsAt":20}})).unwrap(),
    );
    assert_eq!(limits.signal_at(0).unwrap().until, Some(20));
}
