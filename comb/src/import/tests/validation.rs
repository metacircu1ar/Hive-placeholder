use super::*;
use serde_json::json;

#[test]
fn invalid_later_page_and_immutable_timeline_refuse_before_batch_acceptance() {
    let mut fixture = Fixture::new();
    fs::write(
        fixture.source.path().join("c.md"),
        "---\ntitle: [invalid\n---\n",
    )
    .unwrap();
    assert!(!fixture.apply().unwrap_err().file_committed);
    assert!(fixture.comb.import_status().unwrap().current.is_none());
    assert!(!fixture.page("a").exists());
    fs::write(fixture.source.path().join("c.md"), "c").unwrap();
    let c: Slug = "c".parse().unwrap();
    fixture
        .comb
        .put_markdown(&c, "---\ntitle: C\n---\nSummary", None, &write("seed"))
        .unwrap();
    let existing = fixture
        .comb
        .append(&c, "Retain evidence", "test", None, None, &write("append"))
        .unwrap();
    assert!(!fixture.apply().unwrap_err().file_committed);
    assert!(fixture.comb.import_status().unwrap().current.is_none());
    assert!(!fixture.page("a").exists());
    assert_eq!(fixture.comb.get(&c).unwrap().hash, existing.hash);
}

#[test]
fn journal_checks_exact_shapes_bounds_and_identity_before_use() {
    let mut fixture = Fixture::new();
    fixture.pending(1);
    let original: serde_json::Value = serde_json::from_slice(&fixture.journal()).unwrap();
    let mut cases = Vec::new();
    for (pointer, value) in [
        ("/state", json!("old")),
        ("/batch/comb", json!(uuid::Uuid::new_v4().to_string())),
        ("/batch/id", json!("not-a-uuid")),
        ("/batch/source/directory", json!("relative")),
        ("/batch/entries/0/request_id", json!("different")),
        ("/batch/entries/0/fingerprint", json!("f".repeat(64))),
        ("/batch/entries/0/input/relative", json!("../escape.md")),
        ("/batch/entries/0/input/slug", json!("other")),
        ("/batch/entries/0/input/markdown", json!("malformed")),
    ] {
        let mut bad = original.clone();
        *bad.pointer_mut(pointer).unwrap() = value;
        cases.push(bad);
    }
    let mut extra = original.clone();
    extra["version"] = json!(1);
    cases.push(extra);
    let mut missing = original.clone();
    missing["batch"].as_object_mut().unwrap().remove("prefix");
    cases.push(missing);
    let mut duplicate = original.clone();
    duplicate["batch"]["entries"][1] = duplicate["batch"]["entries"][0].clone();
    cases.push(duplicate);
    let mut excessive = original.clone();
    excessive["batch"]["entries"] = json!(vec![original["batch"]["entries"][0].clone(); 1001]);
    cases.push(excessive);
    for bad in cases {
        fs::write(
            fixture.root.path().join(".comb/import.json"),
            serde_json::to_vec(&bad).unwrap(),
        )
        .unwrap();
        assert!(fixture.comb.import_status().is_err(), "{bad}");
        assert!(fixture.apply().is_err());
        assert!(!fixture.page("b").exists());
    }
    fs::write(
        fixture.root.path().join(".comb/import.json"),
        serde_json::to_vec(&original).unwrap(),
    )
    .unwrap();
    fixture.apply().unwrap();
}

#[test]
fn matching_receipts_bind_identity_and_payload_but_not_operation_text() {
    let mut fixture = Fixture::new();
    fixture.pending(1);
    let Some(Record::Pending(batch)) = fixture.comb.disk.import_record().unwrap() else {
        panic!()
    };
    let receipts = fs::read_dir(fixture.root.path().join(".comb/receipts"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(receipts.len(), 1);
    let path = receipts[0].clone();
    let saved = fs::read(&path).unwrap();
    for field in ["slug", "fingerprint", "principal", "request_id"] {
        let mut bad: serde_json::Value = serde_json::from_slice(&saved).unwrap();
        bad[field] = match field {
            "fingerprint" => json!("f".repeat(64)),
            "principal" => json!("client:other"),
            _ => json!("other"),
        };
        fs::write(&path, serde_json::to_vec(&bad).unwrap()).unwrap();
        let error = fixture.apply().unwrap_err();
        assert!(error.file_committed);
        assert!(!fixture.page("b").exists());
        assert!(fixture.comb.abandon_import(&batch.id).is_err());
    }
    let mut opaque: serde_json::Value = serde_json::from_slice(&saved).unwrap();
    opaque["operation"] = json!("opaque-historical-operation");
    fs::write(path, serde_json::to_vec(&opaque).unwrap()).unwrap();
    fixture.apply().unwrap();
}

#[test]
fn oversized_journal_is_rejected_before_parsing_or_writing() {
    let mut fixture = Fixture::new();
    let path = fixture.root.path().join(".comb/import.json");
    let file = fs::File::create(path).unwrap();
    file.set_len(journal::MAX_BYTES as u64 + 1).unwrap();
    assert_eq!(
        fixture.comb.import_status().unwrap_err().message,
        Error::file_size_limit().message
    );
    assert!(fixture.apply().is_err());
    assert!(!fixture.page("a").exists());
}

#[test]
fn completed_record_cannot_acknowledge_missing_or_conflicting_receipts() {
    let mut fixture = Fixture::new();
    fixture.pending(1);
    let path = fixture.root.path().join(".comb/import.json");
    let pending = fixture.journal();
    let mut forged: serde_json::Value = serde_json::from_slice(&pending).unwrap();
    forged["state"] = json!("completed");
    fs::write(&path, serde_json::to_vec(&forged).unwrap()).unwrap();
    assert!(fixture.comb.import_status().is_err());
    assert!(fixture.apply().is_err());
    assert!(!fixture.page("b").exists());
    fs::write(path, pending).unwrap();
    fixture.apply().unwrap();
    let receipt = fs::read_dir(fixture.root.path().join(".comb/receipts"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::remove_file(receipt).unwrap();
    assert!(fixture.comb.import_status().is_err());
    assert!(fixture.apply().is_err());
}
