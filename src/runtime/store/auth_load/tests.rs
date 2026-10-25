use super::*;
use crate::runtime::{
    domain::{AgentState, TaskOutcome},
    store::Store,
    test_support::AgentBuilder,
};

fn assert_load(store: &Store, profile: &str, bound_agents: u64, active_turns: u64) {
    assert_eq!(
        store.auth_load(profile).unwrap(),
        AuthLoad {
            bound_agents,
            active_turns
        }
    );
}

#[test]
fn auth_load_is_transactional_persistent_and_follows_bindings_and_terminal_turns() {
    let dir = crate::runtime::test_directory();
    let path = dir.path().join("db");
    let store = Store::open(&path).unwrap();
    let mut a = AgentBuilder::new("a").build();
    a.turn_id = None;
    store.save(&a).unwrap();
    assert_load(&store, "main", 1, 0);
    store.begin().unwrap();
    let mut b = a.clone();
    b.id = "b".into();
    b.turn_id = Some("turn".into());
    store.save(&b).unwrap();
    assert_load(&store, "main", 2, 1);
    store.rollback().unwrap();
    assert_load(&store, "main", 1, 0);
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_load(&store, "main", 1, 0);
    a.auth = "second".into();
    a.turn_id = Some("turn".into());
    store.save(&a).unwrap();
    assert_load(&store, "main", 0, 0);
    for state in [AgentState::Stopped, AgentState::Failed] {
        a.state = state;
        a.paused = true;
        store.save(&a).unwrap();
        assert_load(&store, "second", 1, 1);
    }
    a.task_outcome = TaskOutcome::Completed;
    store.save(&a).unwrap();
    // The terminal native turn still counts until it is settled.
    assert_load(&store, "second", 0, 1);
    a.turn_id = None;
    store.save(&a).unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_load(&store, "second", 0, 0);
    let count: i64 = store
        .query_row_cached("SELECT count(*) FROM auth_load", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
    let mut c = AgentBuilder::new("c").build();
    c.turn_id = None;
    store.save(&c).unwrap();
    // Exercise the same authoritative delete that purge uses, without its other state.
    store
        .execute_cached("DELETE FROM agents WHERE id='c'", [])
        .unwrap();
    assert_load(&store, "main", 0, 0);
    for (id, expired) in [("cancelled", false), ("expired", true)] {
        let mut agent = AgentBuilder::new(id).build();
        agent.turn_id = None;
        store.save(&agent).unwrap();
        agent.expired = expired;
        if !expired {
            agent.task_outcome = TaskOutcome::Cancelled;
        }
        store.save(&agent).unwrap();
        store.save(&agent).unwrap();
        assert_eq!(store.auth_load("main").unwrap().bound_agents, 0);
    }
}

#[test]
fn ordinary_turns_do_not_write_the_bound_agent_counter() {
    let dir = crate::runtime::test_directory();
    let store = Store::open(&dir.path().join("db")).unwrap();
    let mut agent = AgentBuilder::new("a").build();
    agent.turn_id = None;
    store.save(&agent).unwrap();
    store.sql().execute_batch("CREATE TEMP TRIGGER reject_load_update BEFORE UPDATE ON auth_load BEGIN SELECT RAISE(ABORT,'unexpected counter update'); END;
        CREATE TEMP TRIGGER reject_load_insert BEFORE INSERT ON auth_load BEGIN SELECT RAISE(ABORT,'unexpected counter insert'); END;
        CREATE TEMP TRIGGER reject_load_delete BEFORE DELETE ON auth_load BEGIN SELECT RAISE(ABORT,'unexpected counter delete'); END;").unwrap();
    for turn in [Some("turn"), None, Some("another"), None] {
        agent.turn_id = turn.map(str::to_owned);
        store.save(&agent).unwrap();
        assert_eq!(
            store.auth_load("main").unwrap(),
            AuthLoad {
                bound_agents: 1,
                active_turns: u64::from(turn.is_some())
            }
        );
    }
}

#[test]
fn auth_load_reads_use_the_bound_counter_and_active_partial_index() {
    let dir = crate::runtime::test_directory();
    let store = Store::open(&dir.path().join("db")).unwrap();
    for sql in [
        "EXPLAIN QUERY PLAN SELECT bound_agents FROM auth_load WHERE profile=?1",
        "EXPLAIN QUERY PLAN SELECT count(*) FROM agents WHERE auth=?1 AND turn_id IS NOT NULL",
    ] {
        let plan: Vec<String> = store
            .sql()
            .cached()
            .query_map(sql, ["main"], |r| r.get(3))
            .unwrap();
        assert!(
            plan.iter()
                .any(|line| line.contains("INDEX") && line.contains("auth")),
            "{plan:?}"
        );
        assert!(
            !plan.iter().any(|line| line.starts_with("SCAN ")),
            "{plan:?}"
        );
    }
}
