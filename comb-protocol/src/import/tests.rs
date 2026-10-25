use super::*;
use crate::{
    error::ErrorCode,
    service::{Operation, api::actions::Capability},
};
use serde_json::json;

fn progress() -> ImportProgress {
    ImportProgress {
        batch_id: ImportId::generate(),
        state: ImportState::Pending,
        total: 3,
        completed: 1,
        unfinished: 2,
        unresolved: Some("b".parse().unwrap()),
    }
}

#[test]
fn partial_error_roundtrip_preserves_code_committed_and_bounded_progress() {
    let mut error = Error::io(std::io::Error::other("private native cause")).committed();
    error.import_progress = Some(Box::new(progress()));
    let encoded = serde_json::to_string(&error).unwrap();
    assert!(!encoded.contains("private"));
    let decoded: Error = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded.code, ErrorCode::IoError);
    assert!(decoded.file_committed);
    assert_eq!(decoded.import_progress, error.import_progress);
    for field in [
        "batch_id",
        "state",
        "total",
        "completed",
        "unfinished",
        "unresolved",
    ] {
        let mut missing = serde_json::to_value(&error).unwrap();
        missing["import_progress"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(serde_json::from_value::<Error>(missing).is_err());
    }
    let mut null = serde_json::to_value(&error).unwrap();
    null["import_progress"] = json!(null);
    assert!(serde_json::from_value::<Error>(null).is_err());
}

#[test]
fn contradictory_or_unbounded_import_progress_is_rejected() {
    for (key, value) in [
        ("total", json!(1001)),
        ("completed", json!(4)),
        ("unfinished", json!(3)),
        ("state", json!("completed")),
        ("state", json!("abandoned")),
        ("source", json!("private path")),
        ("batch_id", json!("bad")),
    ] {
        let mut bad = serde_json::to_value(progress()).unwrap();
        bad[key] = value;
        assert!(serde_json::from_value::<ImportProgress>(bad).is_err());
    }
    assert!(serde_json::from_value::<ImportStatus>(json!({})).is_err());
    assert_eq!(
        serde_json::from_value::<ImportStatus>(json!({"current":null}))
            .unwrap()
            .current,
        None
    );
    let id = ImportId::generate();
    for completed in [json!([]), json!(["a", "a"]), json!(["a", "b", "c"])] {
        assert!(
            serde_json::from_value::<ImportReport>(
                json!({"batch_id":id,"total":2,"completed":completed,"state":"completed"})
            )
            .is_err()
        );
    }
    let uppercase = "12345678-1234-4234-8234-123456789ABC";
    assert!(uppercase.parse::<ImportId>().is_err());
}

#[test]
fn import_administration_requires_human_authority_and_strict_arguments() {
    for operation in [
        Operation::ImportStatus,
        Operation::ImportAbandon {
            batch_id: ImportId::generate(),
        },
    ] {
        assert_eq!(operation.required(), Capability::Human);
        let wire = serde_json::to_string(&operation).unwrap();
        assert_eq!(
            serde_json::to_string(&serde_json::from_str::<Operation>(&wire).unwrap()).unwrap(),
            wire
        );
    }
    assert!(!Operation::ImportStatus.is_mutation());
    assert!(
        Operation::ImportAbandon {
            batch_id: ImportId::generate()
        }
        .is_mutation()
    );
    for bad in [
        json!({"op":"import_status", "directory":"private"}),
        json!({"op":"import_abandon"}),
        json!({"op":"import_abandon","batch_id":"bad"}),
    ] {
        let wire = serde_json::to_vec(&json!({
            "protocol": 1,
            "credential": {"comb_id": ImportId::generate(), "id":"human", "secret":"a".repeat(64)},
            "operation": bad
        }))
        .unwrap();
        assert!(crate::service::input::request(&wire).is_err());
    }
}
