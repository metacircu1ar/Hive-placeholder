use super::*;
use crate::runtime::test_support::{RuntimeFixture, failure_log};

#[tokio::test]
async fn stop_record_and_cleanup_failures_keep_the_turn_cause_and_recovery_fence() {
    let fixture = RuntimeFixture::new();
    fixture.hub.with(|core| {
        core.store().test_connection().execute_batch(
            "CREATE TRIGGER fail_stop BEFORE INSERT ON events WHEN NEW.kind = 'provider_session_released' BEGIN SELECT RAISE(ABORT, 'private stop failure'); END;"
        ).map_err(|cause| Error::database_error("fixture trigger").with_source(cause))
    }).await.unwrap();
    let result = finish_execution(
        ExecutionHandle::new(&fixture.hub, "agent", "execution"),
        Err(Error::provider_io("private turn failure")
            .with_source(std::io::Error::from_raw_os_error(libc::EPIPE))),
        super::super::provider::Shutdown {
            method: ShutdownMethod::SessionReleased,
            confirmed: false,
            quiescent: false,
            reaped: false,
            exit_code: None,
        },
        false,
    )
    .await;
    let primary = result.unwrap_err();
    assert_eq!(primary.code(), "provider_io");
    assert_eq!(primary.io_raw_os_error(), Some(libc::EPIPE));
    fixture
        .hub
        .with(move |core| {
            // The failed stop transaction rolled back; uncertain cleanup must still
            // fence the surviving execution even though its original error wins.
            assert_eq!(core.store().agent("agent")?.state, AgentState::Running);
            assert!(!record_failure(core, "agent", primary, false)?);
            let agent = core.store().agent("agent")?;
            assert_eq!(agent.state, AgentState::Failed);
            assert!(agent.recovery_fence.is_some());
            assert_eq!(agent.error.as_deref(), Some("provider_io"));
            Ok(())
        })
        .await
        .unwrap();
    fixture.finish();
}

#[test]
fn failed_lifecycle_write_retains_both_causes_and_fences_without_writer_failure() {
    let Some(log) = failure_log(
        module_path!(),
        "failed_lifecycle_write_retains_both_causes_and_fences_without_writer_failure",
        || {
            let fixture = RuntimeFixture::new();
            fixture.hub.blocking_with(|core| {
            core.store().test_connection().execute_batch(
                "CREATE TRIGGER fail_lifecycle BEFORE INSERT ON events BEGIN SELECT RAISE(ABORT, 'secret lifecycle SQL'); END;"
            ).map_err(|cause| Error::database_error("fixture trigger").with_source(cause))
        }).unwrap();
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(finish_failure(
                &fixture.hub,
                "agent".into(),
                Error::provider_request_failed("secret provider message"),
                true,
            ));
            assert_eq!(fixture.hub.wake.failure().unwrap().code(), "database_error");
            fixture
                .hub
                .blocking_with(|core| {
                    assert!(core.shutdown().load(Ordering::Acquire));
                    // The individual job rolled back cleanly; the writer itself still works.
                    assert_eq!(core.store().agent("agent")?.state, AgentState::Running);
                    Ok(())
                })
                .unwrap();
            fixture.finish();
        },
    ) else {
        return;
    };
    assert!(log.contains("worker.execution_failed"));
    assert!(log.contains("worker.failure_record_failed"));
    assert!(log.contains("provider_request_failed"));
    assert!(log.contains("database_error"));
    assert!(!log.contains("secret lifecycle SQL"));
    assert!(!log.contains("secret provider message"));
}
