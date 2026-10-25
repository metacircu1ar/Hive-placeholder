use super::*;

#[test]
fn memory_temporaries_preserve_large_nested_rollback_and_durable_commits() {
    const MESSAGES: i64 = 64;
    let temp = crate::runtime::test_directory();
    let path = temp.path().join("db");
    let store = Store::open(&path).unwrap();
    assert_production_pragmas(&store);
    assert_eq!(temporary_mode(&store), 0);
    store.use_memory_temporaries().unwrap();
    assert_eq!(temporary_mode(&store), 2);
    assert_production_pragmas(&store);

    // Four MiB of existing message pages exceeds the old 64 KiB journal spill
    // threshold. Exercise multiple undo levels, not just append-only inserts.
    let original = "o".repeat(64 * 1024);
    let outer = "a".repeat(original.len());
    let inner = "b".repeat(original.len());
    store.begin().unwrap();
    store.save(&agent("a", None)).unwrap();
    for i in 0..MESSAGES {
        store
            .enqueue(&format!("message-{i}"), "human", "a", &original)
            .unwrap();
    }
    store.commit().unwrap();
    let check = |expected: &str| {
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT count(*) FROM messages WHERE body=?1",
                    [expected],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            MESSAGES
        );
    };
    let update = |body: &str| {
        store
            .connection
            .execute("UPDATE messages SET body=?1", [body])
            .unwrap();
    };
    store.begin().unwrap();
    update(&outer);
    store.begin().unwrap();
    update(&inner);
    assert!(
        store
            .enqueue("message-0", "human", "a", "duplicate")
            .is_err()
    );
    store.rollback().unwrap();
    check(&outer);
    store.begin().unwrap();
    update(&inner);
    store.commit().unwrap();
    check(&inner);
    store.rollback().unwrap();
    check(&original);

    store.begin().unwrap();
    store.begin().unwrap();
    update(&inner);
    store.commit().unwrap();
    store.commit().unwrap();
    check(&inner);
    let reopened = Store::open(&path).unwrap();
    // The pragma is connection-local and must not affect offline compaction.
    assert_eq!(temporary_mode(&reopened), 0);
    assert_production_pragmas(&reopened);
    assert_eq!(reopened.next_message("a").unwrap().unwrap().body, inner);
    assert_eq!(reopened.active_count().unwrap(), 1);
}

fn temporary_mode(store: &Store) -> i64 {
    store
        .connection
        .query_row("PRAGMA temp_store", [], |row| row.get::<_, i64>(0))
        .unwrap()
}

fn assert_production_pragmas(store: &Store) {
    assert_eq!(
        store
            .connection
            .query_row("PRAGMA synchronous", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        store
            .connection
            .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "wal"
    );

    for (name, expected) in [
        ("foreign_keys", 1),
        ("busy_timeout", 0),
        ("fullfsync", 0),
        ("wal_autocheckpoint", 1000),
    ] {
        let actual: i64 = store
            .connection
            .pragma_query_value(None, name, |row| row.get(0))
            .unwrap();
        assert_eq!(actual, expected, "{name}");
    }
}
