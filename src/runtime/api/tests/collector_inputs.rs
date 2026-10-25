use super::*;
use std::collections::BTreeMap;

fn configured(path: &Path) -> Core {
    let core = core(path);
    core.store()
        .configure_collectors(
            &BTreeMap::from([(
                "shared".into(),
                crate::config::CollectorConfig {
                    credential_file: path.join("unused.json"),
                    enabled: true,
                    sources: [crate::config::CollectionSource::Correction].into(),
                },
            )]),
            &BTreeMap::from([(
                "shared".into(),
                "00000000-0000-4000-8000-000000000001".into(),
            )]),
        )
        .unwrap();
    core
}
fn status(core: &Core) -> Value {
    serde_json::to_value(core.store().collector_status().unwrap()).unwrap()
}
fn assert_input(core: &mut Core, raw: Value, expected_code: &str, field: &str) {
    let before = status(core);
    control_inputs::assert_input(
        call(core, "human", "retry", raw),
        expected_code,
        field,
        "collector_admin",
    );
    assert_eq!(status(core), before);
}

#[test]
fn collector_fields_are_deferred_strict_and_private() {
    let temp = crate::runtime::test_directory();
    let mut core = configured(temp.path());
    for (action, field) in [
        ("rebind", "action"),
        ("discard", "destination"),
        ("release_uncertain", "item"),
        ("rebind", "comb_id"),
    ] {
        for (value, code) in [
            (None, "missing_argument"),
            (Some(json!(null)), "invalid_value"),
            (Some(json!({"private-canary":true})), "invalid_value"),
            (Some(json!(1)), "invalid_value"),
        ] {
            let mut raw = json!({"op":"collector_admin", "action":action, "destination":"shared"});
            raw.as_object_mut().unwrap().remove(field);
            if let Some(value) = value {
                raw[field] = value;
            }
            assert_input(&mut core, raw, code, &format!("/{field}"));
        }
    }
    for action in [
        json!("private-canary"),
        json!({"discard":null}),
        json!(""),
        json!("release-uncertain"),
    ] {
        assert_input(
            &mut core,
            json!({"op":"collector_admin", "action":action}),
            "invalid_value",
            "/action",
        );
    }
    for destination in [json!("private.canary"), json!("x".repeat(65))] {
        assert_input(
            &mut core,
            json!({"op":"collector_admin", "action":"discard", "destination":destination}),
            "invalid_value",
            "/destination",
        );
    }
    for (action, field) in [
        ("discard", "item"),
        ("discard", "comb_id"),
        ("rebind", "item"),
        ("release_uncertain", "comb_id"),
    ] {
        let mut raw = json!({"op":"collector_admin", "action":action});
        raw[field] = Value::Null;
        assert_input(
            &mut core,
            raw,
            "conflicting_arguments",
            &format!("/{field}"),
        );
    }
    assert!(call(&mut core, "human", "unknown", json!({"op":"collector_admin", "action":"discard", "destination":"shared", "unknown":true})).is_err());
}

#[test]
fn collector_destination_lookup_precedes_action_values_and_rejections_never_mutate() {
    use std::error::Error as _;
    let temp = crate::runtime::test_directory();
    let mut core = configured(temp.path());
    for action in ["discard", "release_uncertain", "rebind"] {
        assert_input(
            &mut core,
            json!({"op":"collector_admin", "action":action, "destination":"private-canary"}),
            "invalid_value",
            "/destination",
        );
    }
    for (action, field) in [
        ("discard", "destination"),
        ("release_uncertain", "item"),
        ("rebind", "comb_id"),
    ] {
        for value in [String::new(), "é".repeat(32769)] {
            let mut raw = json!({"op":"collector_admin", "action":action, "destination":"shared"});
            raw[field] = json!(value);
            assert_input(&mut core, raw, "out_of_range", &format!("/{field}"));
        }
    }
    assert_input(
        &mut core,
        json!({"op":"collector_admin", "action":"release_uncertain", "destination":"shared", "item":"private-canary"}),
        "invalid_value",
        "/item",
    );
    let error = call(&mut core, "human", "uuid", json!({"op":"collector_admin", "action":"rebind", "destination":"shared", "comb_id":"private-canary"})).unwrap_err();
    assert!(
        error
            .source()
            .unwrap()
            .downcast_ref::<uuid::Error>()
            .is_some()
    );
    control_inputs::assert_input(Err(error), "invalid_uuid", "/comb_id", "collector_admin");
}

#[test]
fn collector_rebind_works_for_retained_disabled_destinations_and_canonicalizes_uuid() {
    let temp = crate::runtime::test_directory();
    let mut core = configured(temp.path());
    core.store()
        .configure_collectors(&BTreeMap::new(), &BTreeMap::new())
        .unwrap();
    call(&mut core, "human", "rebind", json!({"op":"collector_admin", "action":"rebind", "destination":"shared", "comb_id":"AAAAAAAAAAAA4AAA8AAAAAAAAAAAAAAA"})).unwrap();
    assert_eq!(
        status(&core)[0]["comb_id"],
        "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
    );
    assert_eq!(status(&core)[0]["enabled"], false);
}

#[test]
fn collector_admin_replay_and_authority_precede_current_input_state() {
    let temp = crate::runtime::test_directory();
    let mut core = configured(temp.path());
    let raw = json!({"op":"collector_admin", "action":"discard", "destination":"shared"});
    let first = call(&mut core, "human", "same", raw.clone()).unwrap();
    // New evidence after discard must survive retry of that accepted request.
    call(
        &mut core,
        "human",
        "evidence",
        json!({"op":"evidence", "kind":"correction", "summary":"Public observation."}),
    )
    .unwrap();
    assert_eq!(status(&core)[0]["pending"], 1);
    assert_eq!(call(&mut core, "human", "same", raw).unwrap(), first);
    assert_eq!(status(&core)[0]["pending"], 1);
    assert_eq!(
        call(
            &mut core,
            "human",
            "same",
            json!({"op":"collector_admin", "action":"discard", "destination":"changed"})
        )
        .unwrap_err()
        .code(),
        "request_conflict"
    );
    core.store().save(&agent("a")).unwrap();
    assert_eq!(
        call(
            &mut core,
            "a",
            "agent",
            json!({"op":"collector_admin", "action":null})
        )
        .unwrap_err()
        .code(),
        "unsupported_operation"
    );
}

#[test]
fn collector_release_replays_after_item_is_no_longer_attempted_and_fences_old_ack() {
    let temp = crate::runtime::test_directory();
    let mut core = configured(temp.path());
    call(
        &mut core,
        "human",
        "evidence",
        json!({"op":"evidence", "kind":"correction", "summary":"Public observation."}),
    )
    .unwrap();
    let destination = core.store().collector_destinations().unwrap().remove(0);
    let item = core
        .store()
        .claim_collection(&destination, "00000000-0000-4000-8000-000000000001", "key")
        .unwrap()
        .unwrap();
    let raw = json!({"op":"collector_admin", "action":"release_uncertain", "destination":"shared", "item":item.id});
    let first = call(&mut core, "human", "release", raw.clone()).unwrap();
    assert_eq!(status(&core)[0]["uncertain"], 0);
    assert_eq!(
        call(&mut core, "human", "release", raw.clone()).unwrap(),
        first
    );
    assert_input(&mut core, raw, "invalid_value", "/item");
    core.store().finish_collection(&item, None).unwrap();
    assert_eq!(
        status(&core)[0]["pending"],
        1,
        "an old acknowledgement cannot delete released work"
    );
}
