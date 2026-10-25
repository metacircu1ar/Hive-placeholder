use super::*;
use serde_json::{Value, json};

fn replace_intent(fixture: &Fixture, bytes: &[u8]) {
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .write(&fixture.paths()[0], bytes)
        .unwrap();
}

fn refused(fixture: Fixture) {
    let paths = fixture.paths();
    let before: Vec<_> = paths.iter().map(fs::read).collect();
    drop(fixture.comb);
    assert!(Comb::open(fixture.root.path()).is_err());
    for (path, before) in paths.iter().zip(before) {
        match before {
            Ok(bytes) => assert_eq!(fs::read(path).unwrap(), bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => assert!(!path.exists()),
            other => panic!("unexpected fixture read: {other:?}"),
        }
    }
}

#[test]
fn malformed_pair_records_are_refused_before_either_page_changes() {
    let mutations: &[fn(&mut Value)] = &[
        |v| {
            v["record"]["note"] = v["record"]["target"].clone();
        },
        |v| {
            v["record"]["target"]["after"] = json!("00".repeat(32));
        },
        |v| {
            v["record"]["note"]["before"] = json!("not-a-hash");
        },
        |v| {
            v["record"]["note"]["receipt"]["slug"] = json!("concepts/other");
        },
        |v| {
            v["record"]["note"]["receipt"]["principal"] = json!("unknown");
        },
        |v| {
            v["record"]["note"]["receipt"]["request_id"] = json!("wrong");
        },
        |v| {
            v["record"]["note"]["receipt"]["fingerprint"] = json!("bad");
        },
        |v| {
            v["record"]["note"]["receipt"]["extra"] = json!(true);
        },
        |v| {
            v["record"]["target"]["extra"] = json!(true);
        },
        |v| {
            v["extra"] = json!(true);
        },
        |v| {
            v["record"].as_object_mut().unwrap().remove("note");
        },
        |v| {
            v["record"]["target"]
                .as_object_mut()
                .unwrap()
                .remove("before");
        },
        |v| {
            v["record"]["target"]["raw"] = json!("x".repeat(page::PAGE_BYTES + 1));
        },
        |v| {
            v["record"]["note"]["receipt"]["operation"] =
                json!("x".repeat(crate::disk::RECORD_BYTES));
        },
        |v| {
            let raw = v["record"]["note"]["raw"]
                .as_str()
                .unwrap()
                .replace("Original evidence", "Forged evidence");
            v["record"]["note"]["after"] = json!(ContentHash::digest(&raw));
            v["record"]["note"]["raw"] = json!(raw);
        },
    ];
    for mutation in mutations {
        let mut fixture = Fixture::new();
        let pair = fixture.pending();
        let mut record = json!({"operation":"consolidation", "record": pair});
        mutation(&mut record);
        replace_intent(&fixture, &serde_json::to_vec(&record).unwrap());
        refused(fixture);
    }
}

#[test]
fn conflicting_claim_and_third_state_source_refuse_before_target_changes() {
    for claim in [false, true] {
        let mut fixture = Fixture::new();
        let pair = fixture.pending();
        if claim {
            let mut receipt = pair.note.receipt.clone();
            receipt.fingerprint = "00".repeat(32);
            fixture.comb.disk.set_receipt(&receipt.into()).unwrap();
        } else {
            atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
                .write(&fixture.paths()[4], b"editor bytes")
                .unwrap();
        }
        refused(fixture);
    }
}

#[test]
fn one_sided_receipts_without_an_intent_are_not_repaired_by_replay() {
    for slot in [3, 6] {
        let mut fixture = Fixture::new();
        fixture.apply().unwrap();
        fs::remove_file(&fixture.paths()[slot]).unwrap();
        let error = fixture.apply().unwrap_err();
        assert_eq!(error.code, ErrorCode::RecoveryRequired);
        assert_eq!(error.message, Error::incomplete_consolidation().message);
        assert!(!fixture.paths()[slot].exists());
        assert!(!fixture.paths()[0].exists());
    }
}

#[test]
fn duplicate_fields_old_shapes_and_each_encoded_record_limit_are_refused() {
    for variant in 0..5 {
        let mut fixture = Fixture::new();
        fixture.pending();
        let original = fs::read_to_string(&fixture.paths()[0]).unwrap();
        let encoded = match variant {
            0 => original.replacen("\"operation\":", "\"operation\":\"consolidation\",\"operation\":", 1),
            1 => original.replacen("\"note\":", "\"note\":null,\"note\":", 1),
            2 => serde_json::to_string(&json!({"slug":"concepts/target", "before":null, "previous_head":null, "after":null, "receipt":null})).unwrap(),
            3 => format!("{}{}", " ".repeat(MAX_BYTES), original),
            4 => format!("{}{}", " ".repeat(crate::disk::RECORD_BYTES), json!({"operation":"single","record":{"slug":"concepts/target", "before":null, "previous_head":null, "after":null, "receipt":null}})),
            _ => unreachable!(),
        };
        replace_intent(&fixture, encoded.as_bytes());
        refused(fixture);
    }
}

#[test]
fn worst_case_escaped_page_fits_the_pair_record_without_raising_decoded_limits() {
    let mut fixture = Fixture::new();
    let mut pair = fixture.pending();
    let mut page = pair.target.page().unwrap();
    page.compiled = "\u{0001}".repeat(page::PAGE_BYTES - pair.target.raw.len() - 1024);
    page.promote_summary(Actor::Human).unwrap();
    pair.target = PreparedPage::new(
        &fixture.comb.disk,
        &pair.target.slug,
        &pair.target.before,
        page,
        pair.target.receipt.clone(),
    )
    .unwrap();
    let encoded =
        crate::error::json_bytes(&super::super::super::RecordRef::Consolidation(&pair)).unwrap();
    assert!(encoded.len() > 5 * page::PAGE_BYTES);
    assert!(encoded.len() < MAX_BYTES);
    assert!(pair.target.raw.len() < page::PAGE_BYTES);
    replace_intent(&fixture, &encoded);
    fixture.recover(Some(pair));
}
