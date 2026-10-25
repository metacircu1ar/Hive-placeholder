use super::super::events::INLINE_EVENT_BYTES;
use super::*;

#[test]
fn unsigned_row_decoders_reject_negative_and_noninteger_values() {
    let db = Connection::open_in_memory().unwrap();
    for value in [-1, i64::MIN] {
        assert!(
            db.query_row("SELECT ?1", [value], |row| rows::unsigned(row, 0))
                .is_err()
        );
        assert!(
            db.query_row("SELECT ?1", [value], |row| rows::optional_unsigned(row, 0))
                .is_err()
        );
        assert!(
            db.query_row("SELECT ?1", [value], |row| rows::count(row, 0))
                .is_err()
        );
        assert!(
            db.query_row("SELECT ?1", [value], |row| rows::nonnegative(row, 0))
                .is_err()
        );
    }
    for value in [0, i64::MAX] {
        assert_eq!(
            db.query_row("SELECT ?1", [value], |row| rows::unsigned(row, 0))
                .unwrap(),
            value.unsigned_abs()
        );
        assert_eq!(
            db.query_row("SELECT ?1", [value], |row| rows::optional_unsigned(row, 0))
                .unwrap(),
            Some(value.unsigned_abs())
        );
        assert_eq!(
            db.query_row("SELECT ?1", [value], |row| rows::nonnegative(row, 0))
                .unwrap(),
            value
        );
    }
    assert_eq!(
        db.query_row("SELECT NULL", [], |row| rows::optional_unsigned(row, 0))
            .unwrap(),
        None
    );
    for sql in ["SELECT NULL", "SELECT 1.0", "SELECT '1'"] {
        assert!(db.query_row(sql, [], |row| rows::unsigned(row, 0)).is_err());
        assert!(db.query_row(sql, [], |row| rows::count(row, 0)).is_err());
    }
}

#[test]
fn claims_reject_invalid_classes_without_claiming_an_agent() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    store.save(&agent("a", None)).unwrap();
    store.enqueue("input", "human", "a", "body").unwrap();
    for class in [4, usize::MAX] {
        assert_eq!(store.claim(class).unwrap_err().code(), "database_error");
        assert!(
            !store
                .connection
                .query_row("SELECT claimed FROM agents WHERE id='a'", [], |r| r
                    .get::<_, bool>(0))
                .unwrap()
        );
    }
    let claims = (0..4)
        .filter_map(|class| store.claim(class).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(claims, ["a"]);
    assert!((0..=2).contains(&store.placement("a").unwrap()));
}

#[test]
fn oversized_saved_numbers_are_errors_without_overwriting_the_record() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    let original = agent("a", None);
    store.save(&original).unwrap();
    let saved = api::value(store.agent("a").unwrap());
    for field in [
        "auth_checked_revision",
        "provider_spawn_before",
        "provider_spawn_after",
    ] {
        let mut value = api::value(&original);
        value["provider_id"] = json!("provider");
        value["provider_pid"] = json!(123);
        value["provider_spawn_before"] = json!(1);
        value["provider_spawn_after"] = json!(2);
        value[field] = json!(u64::MAX);
        let invalid: Agent = serde_json::from_value(value).unwrap();
        assert_eq!(store.save(&invalid).unwrap_err().code(), "database_error");
        assert_eq!(api::value(store.agent("a").unwrap()), saved);
    }
    let mut ended = original;
    ended.task_outcome = TaskOutcome::Completed;
    assert_eq!(
        store.save_at(&ended, u64::MAX).unwrap_err().code(),
        "database_error"
    );
    assert_eq!(api::value(store.agent("a").unwrap()), saved);
    store.save_at(&ended, i64::MAX.unsigned_abs()).unwrap();
    assert_eq!(
        store.agent("a").unwrap().ended_at,
        Some(i64::MAX.unsigned_abs())
    );
}

#[test]
fn history_reads_reject_negative_timestamps_and_retention_cursors() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    let agent = agent("a", None);
    store.save(&agent).unwrap();
    store.event(&agent, EventKind::Spawned, json!({})).unwrap();
    store
        .connection
        .execute("INSERT INTO event_retention VALUES('a',-1)", [])
        .unwrap();
    assert_eq!(
        store.events("a", 0, 100).unwrap_err().code(),
        "database_error"
    );
    store
        .connection
        .execute("DELETE FROM event_retention", [])
        .unwrap();
    store
        .connection
        .execute("UPDATE events SET at=-1", [])
        .unwrap();
    assert_eq!(
        store.events("a", 0, 100).unwrap_err().code(),
        "database_error"
    );
    store
        .connection
        .execute("UPDATE events SET at=0", [])
        .unwrap();
    assert_eq!(
        store.events("a", u64::MAX, 200).unwrap()["cursor"],
        json!(u64::MAX)
    );
    assert_eq!(
        store.agent_page(u64::MAX, 200, None, None).unwrap()["agents"],
        json!([])
    );
    assert_eq!(
        store.events("a", 0, 200).unwrap()["events"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn artifact_reads_and_manifest_rows_reject_negative_metadata() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    let agent = agent("a", None);
    store.save(&agent).unwrap();
    store
        .event(
            &agent,
            EventKind::AssistantMessage,
            json!({"text":"x".repeat(INLINE_EVENT_BYTES+1)}),
        )
        .unwrap();
    let events = store.events("a", 0, 100).unwrap();
    let id = events["events"][0]["data"]["artifact"]["id"]
        .as_str()
        .unwrap();
    assert!(
        store.output(id, 0, 65536).unwrap()["bytes"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert_eq!(
        store.output(id, u64::MAX, 65536).unwrap_err().code(),
        "artifact_unavailable"
    );
    store
        .connection
        .execute("UPDATE artifacts SET created_at=-1", [])
        .unwrap();
    assert!(
        store
            .connection
            .query_row(queries::ARTIFACT_MANIFEST, [], rows::Artifact::read)
            .is_err()
    );
    store
        .connection
        .execute("UPDATE artifacts SET created_at=0,bytes=-1", [])
        .unwrap();
    assert!(
        store
            .connection
            .query_row(queries::ARTIFACT_MANIFEST, [], rows::Artifact::read)
            .is_err()
    );
    assert_eq!(
        store.output(id, 0, 100).unwrap_err().code(),
        "database_error"
    );
}

#[test]
fn message_reads_reject_negative_schedule_sequence_and_start_cursor() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    store.save(&agent("a", None)).unwrap();
    store.enqueue("m", "human", "a", "body").unwrap();
    store.track_call("m").unwrap();
    assert_eq!(
        store.messages("a", Some(u64::MAX), 200).unwrap()["messages"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    store
        .connection
        .execute("UPDATE call_results SET start_seq=-1", [])
        .unwrap();
    assert_eq!(
        store.message_result("m").unwrap_err().code(),
        "database_error"
    );
    store
        .connection
        .execute("UPDATE call_results SET start_seq=NULL", [])
        .unwrap();
    store
        .connection
        .execute("UPDATE messages SET seq=-1", [])
        .unwrap();
    assert_eq!(
        store.message_result("m").unwrap_err().code(),
        "database_error"
    );
    assert_eq!(
        store.messages("a", None, 100).unwrap_err().code(),
        "database_error"
    );
    store
        .connection
        .execute("UPDATE messages SET seq=1", [])
        .unwrap();
    store
        .connection
        .pragma_update(None, "ignore_check_constraints", true)
        .unwrap();
    store
        .connection
        .execute("UPDATE messages SET not_before=-1", [])
        .unwrap();
    store
        .connection
        .pragma_update(None, "ignore_check_constraints", false)
        .unwrap();
    assert_eq!(
        store.message_result("m").unwrap_err().code(),
        "database_error"
    );
    assert_eq!(
        store.messages("a", None, 100).unwrap_err().code(),
        "database_error"
    );
}
