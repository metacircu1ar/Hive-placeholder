use super::*;
use serde_json::json;

#[test]
fn private_head_keeps_contextual_validation_order() {
    let root = tempfile::tempdir().unwrap();
    let disk = Disk::acquire(root.path(), true).unwrap();
    let slug: Slug = "concepts/page".parse().unwrap();
    let path = disk.head_path(&slug);
    atomic(
        &path,
        &crate::error::json_bytes(&json!({"slug":"../wrong","hash":"bad"})).unwrap(),
        &System,
    )
    .unwrap();
    assert_eq!(
        disk.head(&slug).unwrap_err().message,
        Error::receipt_slug_mismatch().message
    );
    atomic(
        &path,
        &crate::error::json_bytes(&json!({"slug":slug,"hash":"bad"})).unwrap(),
        &System,
    )
    .unwrap();
    assert_eq!(
        disk.head(&slug).unwrap_err().message,
        Error::invalid_content_digest().message
    );
    atomic(
        &path,
        &crate::error::json_bytes(&json!({"slug":slug})).unwrap(),
        &System,
    )
    .unwrap();
    assert_eq!(
        disk.head(&slug).unwrap_err().code,
        crate::ErrorCode::InvalidJson
    );
    atomic(
        &path,
        &crate::error::json_bytes(&json!({"slug":slug,"hash":null})).unwrap(),
        &System,
    )
    .unwrap();
    assert!(disk.head(&slug).unwrap().is_none());
}

#[test]
fn private_intent_keeps_contextual_validation_order() {
    let root = tempfile::tempdir().unwrap();
    let disk = Disk::acquire(root.path(), true).unwrap();
    let slug: Slug = "concepts/page".parse().unwrap();
    atomic(
        &disk.head_path(&slug),
        &crate::error::json_bytes(&json!({"slug":slug,"hash":null})).unwrap(),
        &System,
    )
    .unwrap();
    let intent = disk.admin.join("intent.json");
    for field in ["before", "previous_head", "after"] {
        let mut raw = json!({"slug":"../wrong","before":null,"previous_head":null,"after":null,"receipt":null});
        raw[field] = json!("bad");
        atomic(
            &intent,
            &crate::error::json_bytes(&json!({"operation":"single","record":raw})).unwrap(),
            &System,
        )
        .unwrap();
        assert_eq!(
            disk.recover().unwrap_err().message,
            Error::invalid_content_digest().message
        );
        raw[field] = json!(null);
        atomic(
            &intent,
            &crate::error::json_bytes(&json!({"operation":"single","record":raw})).unwrap(),
            &System,
        )
        .unwrap();
        assert_eq!(
            disk.recover().unwrap_err().code,
            crate::ErrorCode::InvalidSlug
        );
    }
    for field in ["before", "previous_head", "after", "receipt"] {
        let mut raw =
            json!({"slug":slug,"before":null,"previous_head":null,"after":null,"receipt":null});
        raw.as_object_mut().unwrap().remove(field);
        atomic(
            &intent,
            &crate::error::json_bytes(&json!({"operation":"single","record":raw})).unwrap(),
            &System,
        )
        .unwrap();
        assert_eq!(
            disk.recover().unwrap_err().code,
            crate::ErrorCode::InvalidJson
        );
    }
}

#[test]
fn receipt_identity_precedes_slug_and_fingerprint_is_a_raw_claim() {
    let root = tempfile::tempdir().unwrap();
    let disk = Disk::acquire(root.path(), true).unwrap();
    let principal = Principal::for_actor(&crate::Actor::Human).unwrap();
    let request: RequestId = "request".parse().unwrap();
    let path = disk
        .receipt_path(principal.as_str(), request.as_str())
        .unwrap();
    let mut raw = json!({"principal":"wrong","request_id":request,"fingerprint":"malformed","operation":"opaque","slug":"../wrong"});
    atomic(&path, &crate::error::json_bytes(&raw).unwrap(), &System).unwrap();
    assert_eq!(
        disk.receipt(&principal, &request).unwrap_err().message,
        Error::receipt_identity_mismatch().message
    );
    raw["principal"] = json!(principal);
    raw["request_id"] = json!("wrong");
    atomic(&path, &crate::error::json_bytes(&raw).unwrap(), &System).unwrap();
    assert_eq!(
        disk.receipt(&principal, &request).unwrap_err().message,
        Error::receipt_identity_mismatch().message
    );
    raw["request_id"] = json!(request);
    atomic(&path, &crate::error::json_bytes(&raw).unwrap(), &System).unwrap();
    assert_eq!(
        disk.receipt(&principal, &request).unwrap_err().code,
        crate::ErrorCode::InvalidSlug
    );
    raw["slug"] = json!("concepts/page");
    atomic(&path, &crate::error::json_bytes(&raw).unwrap(), &System).unwrap();
    assert_eq!(
        disk.receipt(&principal, &request)
            .unwrap()
            .unwrap()
            .fingerprint,
        "malformed"
    );
}

#[test]
fn rollback_without_a_receipt_file_does_not_validate_unapplied_claims() {
    let root = tempfile::tempdir().unwrap();
    let disk = Disk::acquire(root.path(), true).unwrap();
    let slug: Slug = "concepts/missing".parse().unwrap();
    let raw = json!({
        "slug":slug,"before":null,"previous_head":null,"after":"ab".repeat(32),
        "receipt":{"principal":"not a principal","request_id":"","fingerprint":"bad","operation":"opaque","slug":"../bad"}
    });
    atomic(
        &disk.admin.join("intent.json"),
        &crate::error::json_bytes(&json!({"operation":"single","record":raw})).unwrap(),
        &System,
    )
    .unwrap();
    assert!(
        matches!(disk.recover().unwrap(), Some(Recovery::Single(recovered)) if recovered == slug)
    );
    assert!(disk.head(&slug).unwrap().is_none());
    disk.finish().unwrap();
    assert!(!disk.admin.join("intent.json").exists());
}

#[test]
fn seals_keep_case_insensitive_hex_and_malformed_means_untrusted() {
    let root = tempfile::tempdir().unwrap();
    let disk = Disk::acquire(root.path(), true).unwrap();
    let slug = "concepts/page".parse().unwrap();
    let mut page = crate::Page::parse("---\ntitle: Page\n---\nText").unwrap();
    disk.seal(&slug, &mut page).unwrap();
    page.install_seal(page.metadata.owned().seal().unwrap().to_uppercase());
    assert!(disk.valid_seal(&slug, &page).unwrap());
    page.install_seal("malformed".into());
    assert!(!disk.valid_seal(&slug, &page).unwrap());
}
