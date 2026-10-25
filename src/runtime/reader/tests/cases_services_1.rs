//! Services regressions.
use super::*;

#[tokio::test]
async fn stalled_agent_reads_without_service_do_not_fall_back_to_writer() {
    let runtime = RuntimeFixture::new();
    crate::test_support::assert_code!(
        runtime
            .hub
            .dispatch(agent_caller("turn"), "read".into(), stalled_request())
            .await,
        "daemon_stopping"
    );
    let ordinary = api::RequestEnvelope::new(json!({"op":"agents"}));
    assert!(
        runtime
            .hub
            .dispatch(agent_caller("turn"), "read".into(), ordinary)
            .await
            .is_ok()
    );
    crate::test_support::assert_code!(
        runtime
            .hub
            .with(|core| core.dispatch(
                &crate::runtime::hub::Caller::Human,
                "read",
                &stalled_request(),
                &crate::runtime::core::preflight::Facts::default()
            ))
            .await,
        "unsupported_operation"
    );
    runtime.finish();
}
