use super::*;
use crate::runtime::test_support::{RuntimeFixture, failing_catalog, failure_log};

#[test]
fn unconfirmed_discovery_retains_cause_capacity_and_fence() {
    let Some(log) = failure_log(
        module_path!(),
        "unconfirmed_discovery_retains_cause_capacity_and_fence",
        || {
            let fixture = RuntimeFixture::new();
            let pool = fixture.pool();
            let state = State::new(fixture.temp.path().join("state")).unwrap();
            let profile = crate::runtime::test_support::agent().profile;
            let binary = failing_catalog(&fixture);
            let error = fixture
                .hub
                .catalogs
                .resolve(BindingInput {
                    credential: None,
                    keys: Arc::default(),
                    auth: "main".into(),
                    profile,
                    generation: fixture.lease.generation(),
                    state,
                    binary,
                    daemon_lock: fixture.daemon_lock.clone(),
                    launcher: fixture.launcher.clone(),
                })
                .err()
                .unwrap();
            assert_eq!(error.code(), "recovery_required");
            assert_eq!(
                error
                    .source()
                    .unwrap()
                    .downcast_ref::<Error>()
                    .unwrap()
                    .code(),
                "provider_request_failed"
            );
            assert!(fixture.hub.catalogs.uncertain.load(Ordering::Acquire));
            assert_eq!(
                fixture.hub.wake.failure().unwrap().code(),
                "recovery_required"
            );
            assert_eq!(pool.registry.lock_or_panic().probes, 1);
            assert_eq!(
                fixture.hub.catalogs.reserve().err().unwrap().code(),
                "daemon_stopping"
            );
            drop(pool);
            fixture.finish();
        },
    ) else {
        return;
    };
    assert!(log.contains("catalog.cleanup_unconfirmed"));
    assert!(log.contains("catalog.reservation_abandoned"));
    assert!(!log.contains("secret native discovery"));
}
