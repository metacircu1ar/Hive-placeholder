use super::*;
use crate::ErrorCode;
use std::panic::{AssertUnwindSafe, catch_unwind};

#[test]
fn exclusive_unwind_fences_before_the_next_acquisition() {
    let health = Arc::new(Health::default());
    let mutex = Mutex::new(7, health.clone());
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            let mut guard = mutex.lock().unwrap();
            *guard = 8;
            panic!("private panic payload");
        }))
        .is_err()
    );
    assert_eq!(
        health.check().unwrap_err().code,
        ErrorCode::RecoveryRequired
    );
    assert_eq!(
        mutex.lock().err().unwrap().code,
        ErrorCode::RecoveryRequired
    );
    let independent = Mutex::new(9, Arc::new(Health::default()));
    assert_eq!(*independent.lock().unwrap(), 9);
}

#[test]
fn writer_unwind_poisons_but_reader_unwind_does_not() {
    let health = Arc::new(Health::default());
    let lock = RwLock::new(7, health.clone());
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            let _guard = lock.read().unwrap();
            panic!("reader fixture");
        }))
        .is_err()
    );
    health.check().unwrap();
    assert_eq!(*lock.write().unwrap(), 7);
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            let _guard = lock.write().unwrap();
            panic!("writer fixture");
        }))
        .is_err()
    );
    assert_eq!(
        health.check().unwrap_err().code,
        ErrorCode::RecoveryRequired
    );
    assert_eq!(lock.read().err().unwrap().code, ErrorCode::RecoveryRequired);
}

#[test]
fn discovering_std_poison_releases_its_guard_before_returning_an_error() {
    let health = Arc::new(Health::default());
    let lock = Mutex::new((), health.clone());
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            let _guard = lock.inner.lock().unwrap();
            panic!("private panic payload");
        }))
        .is_err()
    );
    health.check().unwrap();
    let error = lock.lock().err().unwrap();
    let copy = error.clone();
    assert!(matches!(
        lock.inner.try_lock(),
        Err(sync::TryLockError::Poisoned(_))
    ));
    assert!(!serde_json::to_string(&copy).unwrap().contains("private"));
    assert_eq!(
        std::error::Error::source(&copy).unwrap().to_string(),
        "lock poisoned"
    );
}

#[test]
fn post_acquisition_check_drops_a_guard_when_another_domain_has_failed() {
    let health = Health::default();
    let raw = sync::Mutex::new(());
    let acquired = raw.lock();
    health.fence();
    assert_eq!(
        health.acquired(acquired).unwrap_err().code,
        ErrorCode::RecoveryRequired
    );
    assert!(raw.try_lock().is_ok());
}

#[test]
fn failure_preference_preserves_commit_and_request_metadata() {
    let health = Health::default();
    assert_eq!(
        health.error_or(Error::embeddings_busy()).code,
        ErrorCode::EmbeddingsBusy
    );
    health.fence();
    let mut original = Error::worker_failed().committed();
    original.request_id = Some("request".into());
    let error = health.error_or(original);
    assert_eq!(error.code, ErrorCode::RecoveryRequired);
    assert!(error.file_committed);
    assert_eq!(error.request_id.as_deref(), Some("request"));
    let primary = std::error::Error::source(&error)
        .unwrap()
        .downcast_ref::<Error>()
        .unwrap();
    assert_eq!(primary.code, ErrorCode::WorkerFailed);
}
