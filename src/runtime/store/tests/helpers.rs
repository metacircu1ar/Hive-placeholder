use super::*;

#[test]
fn page_budget_admits_first_oversize_item_and_checks_accumulation() {
    let mut budget = PageBudget::new();
    assert!(budget.accept(PAGE_BYTES + 1).unwrap());
    assert!(!budget.accept(1).unwrap());
    let mut exact = PageBudget::new();
    assert!(exact.accept(PAGE_BYTES).unwrap());
    assert!(!exact.accept(1).unwrap());
    let mut overflow = PageBudget::new();
    assert!(overflow.accept(usize::MAX).unwrap());
    assert_eq!(overflow.accept(1).unwrap_err().code(), "database_error");
}

#[test]
fn atomic_failure_rolls_back_only_its_savepoint_and_preserves_wake_superset() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    store.save(&agent("a", None)).unwrap();
    store.changes();
    store
        .atomic(|store| {
            store.enqueue("outer", "human", "a", "outer")?;
            let error = store
                .atomic(|store| {
                    store.enqueue("inner", "human", "a", "rolled back")?;
                    store.enqueue("outer", "human", "a", "duplicate")
                })
                .unwrap_err();
            assert_eq!(error.code(), "database_error");
            assert_eq!(store.transaction_depth.get(), 1);
            Ok(())
        })
        .unwrap();
    assert_eq!(store.transaction_depth.get(), 0);
    let messages = store.messages("a", None, 200).unwrap();
    assert_eq!(messages["messages"].as_array().unwrap().len(), 1);
    assert_eq!(messages["messages"][0]["id"], "outer");
    assert!(store.changes().agents.contains("a"));
}

#[test]
fn agent_hot_columns_view_and_json_removals_cover_the_same_fields() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    let agent = agent("a", None);
    store.save(&agent).unwrap();
    let raw: String = store
        .connection
        .query_row("SELECT data FROM agents WHERE id='a'", [], |row| row.get(0))
        .unwrap();
    let raw: Value = serde_json::from_str(&raw).unwrap();
    for field in schema::HOT.iter().chain(schema::REMOVED_FIELDS) {
        assert!(raw.get(*field).is_none(), "duplicated field {field}");
    }
    let view: String = store
        .connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='agent_records'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let fields: HashSet<_> = view
        .split("'$")
        .skip(1)
        .map(|part| part.strip_prefix('.').unwrap().split('\'').next().unwrap())
        .collect();
    let expected: HashSet<_> = schema::HOT
        .iter()
        .chain(schema::REMOVED_FIELDS)
        .copied()
        .filter(|name| *name != "instructions")
        .collect();
    assert_eq!(fields, expected);
    assert_eq!(api::value(store.agent("a").unwrap()), api::value(agent));
}
