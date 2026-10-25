use super::Error;
#[cfg(test)]
use super::{Cause, ErrorCode};
use std::io;
#[cfg(test)]
use std::{error::Error as _, sync::Arc};

pub struct Unencodable;
impl serde::Serialize for Unencodable {
    fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("private serializer failure"))
    }
}

#[test]
fn encoding_failures_are_internal_and_parsing_failures_remain_input_errors() {
    for error in [
        super::value(Unencodable).unwrap_err(),
        super::json_bytes(&Unencodable).unwrap_err(),
        super::json_text(&Unencodable).unwrap_err(),
        super::yaml_text(&Unencodable).unwrap_err(),
    ] {
        assert_eq!(error.code, ErrorCode::InternalError);
        assert_eq!(
            error.source().unwrap().to_string(),
            "private serializer failure"
        );
        assert_eq!(error.message, "Cannot encode internal data.");
        assert!(!serde_json::to_string(&error).unwrap().contains("private"));
    }
    let json = Error::from(serde_json::from_str::<u64>("invalid").unwrap_err());
    assert_eq!(json.code, ErrorCode::InvalidJson);
    let yaml = Error::from(serde_yaml_ng::from_str::<u64>("invalid").unwrap_err());
    assert_eq!(yaml.code, ErrorCode::InvalidFrontmatter);
}

#[test]
fn cloning_preserves_private_cause_and_wire_output() {
    let error = Error::from(io::Error::new(io::ErrorKind::PermissionDenied, "fixture"));
    let clone = error.clone();
    assert_eq!(error.code, ErrorCode::IoError);
    assert!(Arc::ptr_eq(
        error.source.as_ref().unwrap(),
        clone.source.as_ref().unwrap()
    ));
    assert_eq!(clone.source().unwrap().to_string(), "fixture");
    let wire = serde_json::to_string(&clone).unwrap();
    assert_eq!(
        wire,
        r#"{"code":"io_error","message":"I/O operation failed (PermissionDenied).","file_committed":false}"#
    );
    let decoded: Error = serde_json::from_str(&wire).unwrap();
    assert_eq!(serde_json::to_string(&decoded).unwrap(), wire);
    assert!(decoded.source().is_none());
    assert_eq!(
        serde_json::from_str::<Error>(&wire.replace("io_error", "future_error"))
            .unwrap()
            .code,
        ErrorCode::UnknownError
    );
}

#[test]
fn public_classifications_do_not_reveal_source_content() {
    for kind in [io::ErrorKind::PermissionDenied, io::ErrorKind::StorageFull] {
        let error = Error::from(io::Error::new(kind, "/private/marker"));
        assert!(error.message.contains(&format!("{kind:?}")));
        assert!(
            !serde_json::to_string(&error)
                .unwrap()
                .contains("/private/marker")
        );
        assert!(error.diagnostic().unwrap().contains("/private/marker"));
    }
    let error = Error::from(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_FULL),
        Some("SELECT private_marker".into()),
    ));
    assert_eq!(
        error.message,
        "Index operation failed (SQLite primary code 13)."
    );
    assert!(
        !serde_json::to_string(&error)
            .unwrap()
            .contains("private_marker")
    );
    assert!(
        error
            .diagnostic()
            .unwrap()
            .contains("SELECT private_marker")
    );
}

#[test]
fn input_errors_retain_coordinates_without_input_text() {
    let json = serde_json::from_str::<u64>("\n\"private-marker\"").unwrap_err();
    let line = json.line();
    let column = json.column();
    let error = Error::from(json);
    assert_eq!(
        error.message,
        format!("Invalid JSON (Data, line {line}, column {column}).")
    );
    assert!(error.diagnostic().unwrap().contains("private-marker"));
    assert!(
        !serde_json::to_string(&error)
            .unwrap()
            .contains("private-marker")
    );
    let yaml = serde_yaml_ng::from_str::<u64>("private-marker").unwrap_err();
    let location = yaml.location().map(|at| (at.line(), at.column()));
    let error = Error::from(yaml);
    assert_eq!(
        error.message,
        location.map_or_else(
            || "Invalid YAML.".to_owned(),
            |(line, column)| format!("Invalid YAML (line {line}, column {column})."),
        )
    );
    assert!(error.diagnostic().unwrap().contains("private-marker"));
    assert!(
        !serde_json::to_string(&error)
            .unwrap()
            .contains("private-marker")
    );
}

#[test]
fn only_requested_file_open_can_classify_absence() {
    let missing = || io::Error::from(io::ErrorKind::NotFound);
    let opening = Error::opening_file(missing());
    let later_check = Error::from(missing());
    let nested = Error::opening_file(io::Error::other(missing()));
    assert!(opening.is_missing_file());
    assert!(!later_check.is_missing_file());
    assert!(!nested.is_missing_file());
    assert_eq!(
        serde_json::to_value(opening).unwrap(),
        serde_json::to_value(later_check).unwrap()
    );
}

#[test]
fn channel_causes_drop_permits_and_payloads_before_errors_are_cloned() {
    let slots = Arc::new(tokio::sync::Semaphore::new(1));
    for closed in [false, true] {
        let payload = Arc::new(());
        let job = (slots.clone().try_acquire_owned().unwrap(), payload.clone());
        let failure = if closed {
            tokio::sync::mpsc::error::TrySendError::Closed(job)
        } else {
            tokio::sync::mpsc::error::TrySendError::Full(job)
        };
        let error = Error::not_found().caused_by(Cause::channel(failure));
        let clone = error.clone();
        assert_eq!(slots.available_permits(), 1);
        assert_eq!(Arc::strong_count(&payload), 1);
        assert_eq!(
            clone.source().unwrap().to_string(),
            if closed {
                "channel closed"
            } else {
                "channel full"
            }
        );
    }
}

#[test]
fn poisoned_cause_drops_the_acquired_guard() {
    let lock = Arc::new(std::sync::Mutex::new(()));
    let worker = lock.clone();
    assert!(
        std::thread::spawn(move || {
            let _guard = worker.lock().unwrap();
            panic!("poison fixture");
        })
        .join()
        .is_err()
    );
    let cause = Cause::poisoned(lock.lock().unwrap_err());
    let error = Error::not_found().caused_by(cause);
    let _clone = error.clone();
    assert!(matches!(
        lock.try_lock(),
        Err(std::sync::TryLockError::Poisoned(_))
    ));
}

#[test]
fn environment_causes_never_retain_rejected_credentials() {
    use std::os::unix::ffi::OsStringExt;
    let secret = std::ffi::OsString::from_vec(b"rejected-secret\xff".to_vec());
    let cause = Cause::environment(std::env::VarError::NotUnicode(secret));
    let error = Error::embedding_credential_missing().caused_by(cause);
    assert!(!format!("{error:?}").contains("rejected-secret"));
    assert_eq!(
        error.source().unwrap().to_string(),
        "environment value is not Unicode"
    );
}

#[test]
fn committed_result_preserves_classification_request_and_private_cause() {
    use super::CommitExt;
    let mut original = Error::from(io::Error::new(io::ErrorKind::PermissionDenied, "fixture"));
    original.request_id = Some("request".into());
    let promoted = Err::<(), _>(original.clone()).committed().unwrap_err();
    assert!(promoted.file_committed);
    assert_eq!(promoted.code, original.code);
    assert_eq!(promoted.message, original.message);
    assert_eq!(promoted.request_id, original.request_id);
    assert!(Arc::ptr_eq(
        promoted.source.as_ref().unwrap(),
        original.source.as_ref().unwrap(),
    ));
    let converted = Err::<(), _>(io::Error::from(io::ErrorKind::StorageFull))
        .committed()
        .unwrap_err();
    assert!(converted.file_committed);
    assert_eq!(converted.code, ErrorCode::IoError);
    assert_eq!(converted.message, "I/O operation failed (StorageFull).");
    assert!(converted.source().is_some());
    assert_eq!(Ok::<_, io::Error>(42).committed().unwrap(), 42);
}

/// Native errors remain available through the standard source contract.
pub fn io_cause(error: &Error) -> &io::Error {
    std::error::Error::source(error)
        .unwrap()
        .downcast_ref()
        .unwrap()
}
