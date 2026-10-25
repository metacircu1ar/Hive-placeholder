use super::*;
use crate::error::Result;
use crate::runtime::{
    Core,
    domain::{AgentState, MessageState},
    test_support::RuntimeFixture,
};
use serde_json::json;

fn limits(raw: serde_json::Value, now: u64) -> signals::CodexLimits {
    let mut limits = signals::CodexLimits::default();
    limits.merge_at(serde_json::from_value(raw).unwrap(), now);
    limits
}
fn read(permission: serde_json::Value) -> signals::CodexRead {
    serde_json::from_value(json!({"ordinaryUsageAllowed":permission,"rateLimits":{"primary":{"usedPercent":0},"secondary":{"usedPercent":20}}})).unwrap()
}

fn assert_stable_and_mixed_deadlines(core: &mut Core) -> Result<()> {
    let a = core.store().agent("agent")?;
    let now = crate::state::now();
    core.auth_signal(
        "main",
        a.auth_generation,
        "provider",
        signals::Signal::limited(AuthReason::UsageLimit),
    )?;
    let original = core.store().availability("main", a.auth_generation)?;
    core.auth_signal(
        "main",
        a.auth_generation,
        "provider",
        signals::Signal::limited(AuthReason::UsageLimit),
    )?;
    let duplicate = core.store().availability("main", a.auth_generation)?;
    assert_eq!(original.revision, duplicate.revision);
    assert_eq!(original.retry_at, duplicate.retry_at);
    assert!(original.retry_at.unwrap() >= now + 3600);
    core.auth_signal(
        "main",
        a.auth_generation,
        "provider",
        signals::Signal {
            reset_floor: Some(now + 7200),
            ..signals::Signal::limited(AuthReason::UsageLimit)
        },
    )?;
    core.auth_signal(
        "main",
        a.auth_generation,
        "provider",
        signals::Signal::limited(AuthReason::Authentication),
    )?;
    let mixed = core.store().availability("main", a.auth_generation)?;
    assert_eq!(mixed.reason, Some(AuthReason::UsageAndAuthentication));
    assert_eq!(mixed.retry_at, Some(now + 7200));
    core.auth_live_read(
        "main",
        a.auth_generation,
        "provider",
        (mixed.revision, None),
        read(json!(true)),
    )?;
    assert_eq!(
        core.store()
            .availability("main", a.auth_generation)?
            .revision,
        mixed.revision
    );
    let second = uuid::Uuid::new_v4();
    core.auth_signal(
        "main",
        second,
        "provider",
        signals::Signal::limited(AuthReason::Authentication),
    )?;
    assert!(
        core.store().availability("main", second)?.retry_at.unwrap() <= crate::state::now() + 300
    );
    core.auth_signal(
        "main",
        second,
        "provider",
        signals::Signal::limited(AuthReason::UsageLimit),
    )?;
    let reverse = core.store().availability("main", second)?;
    assert_eq!(reverse.reason, mixed.reason);
    assert!(reverse.retry_at.unwrap() >= now + 3600);
    Ok(())
}

mod cases_authentication_1;
mod cases_behaviour_1;
mod cases_recovery_1;
mod cases_requests_1;
mod cases_streaming_1;
