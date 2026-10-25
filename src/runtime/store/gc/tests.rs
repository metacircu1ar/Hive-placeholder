use super::queries::CANDIDATE_PAGE;
use super::*;

#[test]
#[allow(clippy::unwrap_used)]
fn gc_page_uses_ended_index_without_sorting() {
    let tmp = crate::runtime::test_directory();
    let store = Store::open(&tmp.path().join("runtime.sqlite")).unwrap();
    let mut statement = store
        .connection
        .prepare(&format!("EXPLAIN QUERY PLAN {CANDIDATE_PAGE}"))
        .unwrap();
    let plan = statement
        .query_map(params![100, 1, "a"], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .join("\n");
    assert!(
        plan.contains("SEARCH agents USING COVERING INDEX agents_ended"),
        "{plan}"
    );
    assert!(!plan.contains("TEMP B-TREE"), "{plan}");
}
