use super::*;
use crate::ErrorCode;
use hive_setup_support::service::RestartFailure;
use std::{error::Error as _, io};

fn restart(operation: Option<io::Error>) -> Error {
    error(io::Error::other(RestartFailure {
        operation,
        restart: io::Error::other("restart failed"),
    }))
}

fn retained_failure(error: &Error) -> &RestartFailure {
    error
        .source()
        .unwrap()
        .downcast_ref::<io::Error>()
        .unwrap()
        .get_ref()
        .unwrap()
        .downcast_ref::<RestartFailure>()
        .unwrap()
}

#[test]
fn restart_failure_preserves_failed_operation_metadata_and_cause() {
    for committed in [false, true] {
        let mut operation = Error::conflict().caused_by(io::Error::from_raw_os_error(libc::ENOSPC));
        operation.file_committed = committed;
        operation.request_id = Some("request-1".into());
        let error = restart(Some(io::Error::other(operation)));
        assert_eq!(error.code, ErrorCode::Conflict);
        assert_eq!(error.file_committed, committed);
        assert_eq!(error.request_id.as_deref(), Some("request-1"));
        assert_eq!(
            error.message,
            "Page changed or already exists; get its current hash before updating.; service restart failed: restart failed"
        );
        let original = retained_failure(&error)
            .operation
            .as_ref()
            .unwrap()
            .get_ref()
            .unwrap()
            .downcast_ref::<Error>()
            .unwrap();
        assert_eq!(
            original
                .source()
                .unwrap()
                .downcast_ref::<io::Error>()
                .unwrap()
                .raw_os_error(),
            Some(libc::ENOSPC)
        );
    }
}

#[test]
fn restart_failure_preserves_committed_and_untyped_operation_outcomes() {
    let committed = restart(None);
    assert_eq!(committed.code, ErrorCode::IoError);
    assert!(committed.file_committed);
    assert!(committed.request_id.is_none());
    assert_eq!(
        committed.message,
        "Root operation committed.; service restart failed: restart failed"
    );
    assert!(retained_failure(&committed).operation.is_none());

    let failed = restart(Some(io::Error::other("operation failed")));
    assert_eq!(failed.code, ErrorCode::IoError);
    assert!(!failed.file_committed);
    assert_eq!(
        failed.message,
        "operation failed; service restart failed: restart failed"
    );
    assert_eq!(
        retained_failure(&failed)
            .operation
            .as_ref()
            .unwrap()
            .to_string(),
        "operation failed"
    );
}

#[test]
fn plain_service_io_preserves_message_and_native_cause() {
    let cause = io::Error::from_raw_os_error(libc::EACCES);
    let expected = cause.to_string();
    let error = error(cause);
    assert_eq!(error.message, expected);
    assert_eq!(
        error
            .source()
            .unwrap()
            .downcast_ref::<io::Error>()
            .unwrap()
            .raw_os_error(),
        Some(libc::EACCES)
    );
}
#[test]
fn service_arguments_persist_explicit_logging_settings() {
    let root = tempfile::tempdir().unwrap();
    let config = hive_log::Config {
        level: tracing::level_filters::LevelFilter::WARN,
        files: 3,
        file_bytes: 2 * 1024 * 1024,
        ..Default::default()
    };
    let arguments = super::serve_arguments(root.path(), &config).unwrap();
    assert_eq!(
        &arguments[2..],
        [
            "serve",
            "--log-level",
            "warn",
            "--log-file-mib",
            "2",
            "--log-files",
            "3"
        ]
    );
    let layout = hive_setup_support::service::Layout {
        home: root.path().into(),
        config: root.path().join("config"),
        state: root.path().join("state"),
        platform: hive_setup_support::service::Platform::current(),
    };
    let spec = hive_setup_support::service::Spec::new(
        &layout,
        super::scope(root.path()).unwrap(),
        std::path::Path::new("/bin/echo"),
        arguments,
        None,
        vec![],
    )
    .unwrap();
    let encoded = hive_setup_support::files::encode(&spec).unwrap();
    let saved: hive_setup_support::service::Spec = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(saved.arguments, spec.arguments);
    let unit = String::from_utf8(saved.render().unwrap()).unwrap();
    assert!(unit.contains("--log-file-mib"));
    assert!(unit.contains("--log-files"));
}
