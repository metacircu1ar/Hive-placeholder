use super::*;
use serde_json::json;

#[test]
fn registry_preserves_exact_spelling_and_rejects_duplicates() {
    let root = tempfile::tempdir().unwrap();
    let disk = Disk::acquire(root.path(), true).unwrap();
    let id = "abcdefab-cdef-4abc-8abc-abcdefabcdef";
    write_registry(
        &disk,
        vec![record(id, "first"), record(&id.to_uppercase(), "second")],
    );
    let records = Registry::open(&disk, Arc::new(Health::default()))
        .unwrap()
        .list()
        .unwrap();
    assert_eq!(records[0].id.as_str(), id);
    assert_eq!(records[1].id.as_str(), id.to_uppercase());
    write_registry(&disk, vec![record(id, "first"), record(id, "second")]);
    assert_eq!(
        Registry::open(&disk, Arc::new(Health::default()))
            .err()
            .unwrap()
            .code,
        crate::ErrorCode::InvalidCredential
    );
}

#[test]
fn stored_credential_identity_and_name_keep_validation_precedence() {
    let root = tempfile::tempdir().unwrap();
    let disk = Disk::acquire(root.path(), true).unwrap();
    write_registry(&disk, vec![record("human", "sentinel")]);
    assert_eq!(
        Registry::open(&disk, Arc::new(Health::default()))
            .err()
            .unwrap()
            .message,
        Error::invalid_credential_identity().message
    );
    write_registry(&disk, vec![record("bad", "")]);
    assert_eq!(
        Registry::open(&disk, Arc::new(Health::default()))
            .err()
            .unwrap()
            .message,
        Error::invalid_credential_name().message
    );
}

#[test]
fn public_credential_accepts_the_human_sentinel() {
    let id = "abcdefab-cdef-4abc-8abc-abcdefabcdef";
    assert!(
        serde_json::from_value::<Credential>(
            json!({"comb_id":id.to_uppercase(),"id":"human","secret":"raw"})
        )
        .unwrap()
        .id
        .is_human()
    );
}

fn record(id: &str, name: &str) -> serde_json::Value {
    json!({
        "id":id,"grant":{"kind":"client","name":name,"reviewer":false},"verifier":vec![0; 32]
    })
}
fn write_registry(disk: &Disk, credentials: Vec<serde_json::Value>) {
    disk::atomic(
        &disk.admin.join("credentials.json"),
        &crate::error::json_bytes(&json!({"credentials":credentials})).unwrap(),
        &crate::disk::System,
    )
    .unwrap();
}

#[test]
fn raw_role_lists_reject_duplicates_bounds_and_controls() {
    for roles in [
        vec!["same".into(), "same".into()],
        vec!["bad\n".into()],
        (0..65).map(|i| format!("role{i}")).collect(),
        vec!["é".repeat(257)],
    ] {
        let grant = Grant::Bridge {
            name: "bridge".into(),
            reviewer_roles: roles,
        };
        assert!(grant.validate().is_err());
    }
    Grant::Bridge {
        name: "é".repeat(256),
        reviewer_roles: (0..64).map(|i| format!("role{i}")).collect(),
    }
    .validate()
    .unwrap();
}

#[test]
fn contextual_grant_validation_keeps_health_before_input_and_native_api_native() {
    let root = tempfile::tempdir().unwrap();
    let disk = Disk::acquire(root.path(), true).unwrap();
    let health = Arc::new(Health::default());
    let registry = Registry::open(&disk, health.clone()).unwrap();
    let bad = Grant::Client {
        name: String::new(),
        reviewer: false,
    };
    assert!(
        registry
            .create(bad.clone())
            .err()
            .unwrap()
            .input()
            .is_none()
    );
    assert!(
        registry
            .create_input(bad.clone())
            .err()
            .unwrap()
            .input()
            .is_some()
    );
    health.fence();
    let error = registry.create_input(bad).err().unwrap();
    assert!(error.input().is_none());
    assert_eq!(error.code, health.check().unwrap_err().code);
}
