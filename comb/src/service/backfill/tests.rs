use super::*;
use std::error::Error as _;

#[test]
fn recording_failure_preserves_primary_and_stops_fenced_worker() {
    let (dir, server, _) = crate::service::tests::setup(None);
    let state = &server.state;
    state.health.fence();
    let path = dir.path().join("diagnostics.jsonl");
    let (backend, guard) = hive_log::start(&path, hive_log::Config::default()).unwrap();
    backend.scope(|| {
        let primary = Error::from(std::io::Error::from_raw_os_error(libc::ECONNRESET));
        let result: Result<Option<MeasuredBatch>> = state.record_backfill(Err(primary));
        let error = result.as_ref().err().unwrap();
        assert_eq!(error.code, ErrorCode::IoError);
        assert_eq!(
            error
                .source()
                .unwrap()
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .raw_os_error(),
            Some(libc::ECONNRESET)
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        assert!(!runtime.block_on(state.finish_backfill(result, &mut BatchSizer::default())));
    });
    assert_eq!(
        guard.finish(Duration::from_secs(1)).state,
        hive_log::Shutdown::Drained
    );
    let log = std::fs::read_to_string(path).unwrap();
    assert!(log.contains("comb.worker_error_unrecorded"));
    assert!(log.contains("ConnectionReset"));
    assert!(log.contains("comb.backfill_stopped"));
    assert!(!log.contains("comb.backfill_retry"));
}
