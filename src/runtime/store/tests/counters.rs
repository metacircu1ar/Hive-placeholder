use super::*;
use rusqlite::OptionalExtension as _;

const COUNTER_ORACLE: &str = concat!(
    "SELECT count(*) FROM agents a WHERE\n",
    "        pending_count != (SELECT count(*) FROM messages WHERE recipient=a.id AND ",
    "state='accepted' AND mode='next-turn')\n",
    "        OR mail_count != (SELECT count(*) FROM messages WHERE recipient=a.id AND ",
    "state='accepted' AND mode='next-turn' AND startup=0)\n",
    "        OR pending_seq IS NOT (SELECT min(seq) FROM messages WHERE recipient=a.id AND ",
    "state='accepted' AND mode='next-turn')\n",
    "        OR head_startup IS NOT (SELECT startup FROM messages WHERE recipient=a.id AND ",
    "state='accepted' AND mode='next-turn' ORDER BY startup DESC,redirect DESC,seq LIMIT ",
    "1)\n",
    "        OR admission_class != CASE\n",
    "            WHEN (SELECT startup FROM messages WHERE recipient=a.id AND ",
    "state='accepted' AND mode='next-turn' ORDER BY startup DESC,redirect DESC,seq LIMIT ",
    "1)=0\n",
    "                THEN CASE WHEN coordinator=1 THEN 0 ELSE 1 END\n",
    "            WHEN parent_id IS NOT NULL THEN 2 ELSE 3 END\n",
    "        OR priority != CASE WHEN coordinator=1 THEN 0\n",
    "            WHEN EXISTS(SELECT 1 FROM messages WHERE recipient=a.id AND ",
    "state='accepted' AND mode='next-turn' AND startup=0) THEN 1 ELSE 2 END\n",
    "        OR runnable IS NOT (claimed=0 AND blocked=0 AND paused=0 AND expired=0 AND ",
    "task_outcome NOT IN ('completed','cancelled')\n",
    "            AND state IN ('starting','idle','stopped') AND control IS NULL AND ",
    "completion_pending IS NULL\n",
    "            AND (state='starting' OR EXISTS(SELECT 1 FROM messages WHERE ",
    "recipient=a.id AND state='accepted' AND mode='next-turn')))",
);

const ACTIVE_COUNT: &str = concat!(
    "SELECT count(*) FROM agents WHERE NOT(task_outcome IN ('completed','cancelled') OR ",
    "expired=1) OR state!='stopped'",
);

const MAILBOX_HEAD: &str = concat!(
    "SELECT id FROM messages WHERE recipient=?1 AND state='accepted' AND mode='next-turn' ",
    "ORDER BY startup DESC,redirect DESC,seq LIMIT 1",
);

fn check(store: &Store) {
    let mismatches: i64 = store
        .connection
        .query_row(COUNTER_ORACLE, [], |r| r.get(0))
        .unwrap();
    assert_eq!(mismatches, 0);
    let active: i64 = store
        .connection
        .query_row(ACTIVE_COUNT, [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        store.active_count().unwrap(),
        usize::try_from(active).unwrap()
    );
}

#[test]
fn fifo_head_moves_modes_delivery_and_sticky_coordinator_survive_counter_shortcuts() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("runtime.sqlite")).unwrap();
    for (id, parent) in [("root", None), ("a", Some("root")), ("b", Some("root"))] {
        store.save(&agent(id, parent)).unwrap();
    }
    for message in ["first", "second", "third"] {
        store
            .enqueue_kind(message, "human", "a", "text", false)
            .unwrap();
    }
    let head = |id| {
        store
            .connection
            .query_row(MAILBOX_HEAD, [id], |row| row.get::<_, String>(0))
            .optional()
            .unwrap()
    };
    assert_eq!(head("a").as_deref(), Some("first"));
    for sql in [
        "UPDATE messages SET state='delivered_to_turn' WHERE id='second'",
        "UPDATE messages SET startup=1 WHERE id='first'",
    ] {
        store.connection.execute(sql, []).unwrap();
        check(&store);
        assert_eq!(head("a").as_deref(), Some("first"));
    }
    assert_eq!(
        store
            .connection
            .query_row(
                "SELECT status FROM deliveries WHERE message_id='second'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "delivered_to_turn"
    );
    store
        .connection
        .execute("UPDATE messages SET mode='steer' WHERE id='first'", [])
        .unwrap();
    check(&store);
    assert_eq!(head("a").as_deref(), Some("third"));
    store
        .connection
        .execute("UPDATE messages SET state='accepted' WHERE id='second'", [])
        .unwrap();
    check(&store);
    assert_eq!(head("a").as_deref(), Some("second"));
    store
        .connection
        .execute(
            "UPDATE messages SET mode='next-turn',recipient='b' WHERE id='first'",
            [],
        )
        .unwrap();
    check(&store);
    assert_eq!(head("b").as_deref(), Some("first"));
    store
        .connection
        .execute("UPDATE messages SET recipient='b' WHERE id='second'", [])
        .unwrap();
    check(&store);
    assert_eq!(head("a").as_deref(), Some("third"));
    store
        .connection
        .execute("DELETE FROM messages WHERE id='first'", [])
        .unwrap();
    check(&store);
    assert_eq!(head("b").as_deref(), Some("second"));
    assert_sticky_coordinator(&store);
}

fn assert_sticky_coordinator(store: &Store) {
    store.save(&agent("child", Some("a"))).unwrap();
    store
        .connection
        .execute("DELETE FROM agents WHERE id='child'", [])
        .unwrap();
    assert_eq!(
        store
            .connection
            .query_row("SELECT coordinator FROM agents WHERE id='a'", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    check(store);
}

#[test]
fn derived_counters_match_recomputed_state_through_mixed_updates_and_rollbacks() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("runtime.sqlite")).unwrap();
    for id in ["a", "b", "c"] {
        store.save(&agent(id, None)).unwrap();
    }
    store.begin().unwrap();
    let mut random = 0x7de4_921a_u64;
    for step in 0..1500 {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        if step % 11 == 0 {
            store.begin().unwrap();
        }
        mutate(&store, random);
        check(&store);
        if step % 11 == 0 {
            if random & 1 == 0 {
                store.rollback().unwrap();
            } else {
                store.commit().unwrap();
            }
            check(&store);
        }
    }
    store.rollback().unwrap();
    check(&store);
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM messages", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn no_op_contributions_do_not_write_counters_and_null_still_rejects() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("runtime.sqlite")).unwrap();
    store.save(&agent("a", None)).unwrap();
    store.begin().unwrap();
    let before = store.connection.total_changes();
    store
        .connection
        .execute("UPDATE agents SET state='idle' WHERE id='a'", [])
        .unwrap();
    assert_eq!(store.connection.total_changes() - before, 1);
    store
        .enqueue_kind("message", "human", "a", "text", true)
        .unwrap();
    let before = store.connection.total_changes();
    store
        .connection
        .execute("UPDATE messages SET startup=0 WHERE id='message'", [])
        .unwrap();
    assert_eq!(store.connection.total_changes() - before, 2); // Message + one net mailbox row.
    let before = store.connection.total_changes();
    store
        .connection
        .execute("UPDATE messages SET startup=0 WHERE id='message'", [])
        .unwrap();
    assert_eq!(store.connection.total_changes() - before, 1);
    store
        .connection
        .execute(
            "UPDATE agents SET state='stopped',task_outcome='open',expired=0 WHERE id='a'",
            [],
        )
        .unwrap();
    assert!(
        store
            .connection
            .execute("UPDATE agents SET task_outcome=NULL WHERE id='a'", [])
            .is_err()
    );
    check(&store);
    store.rollback().unwrap();
    check(&store);
}

#[test]
fn invalid_head_metadata_rejects_atomically_and_recovery_recomputes_it() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("runtime.sqlite")).unwrap();
    store.save(&agent("a", None)).unwrap();
    store
        .enqueue_kind("seed", "human", "a", "start", true)
        .unwrap();
    store.enqueue("reply", "human", "a", "reply").unwrap();
    for sql in [
        "UPDATE agents SET head_startup=NULL WHERE id='a'",
        "UPDATE agents SET head_startup=2 WHERE id='a'",
        "UPDATE agents SET pending_seq=NULL WHERE id='a'",
        "UPDATE messages SET startup=NULL WHERE id='seed'",
        "UPDATE messages SET startup=2 WHERE id='seed'",
        "UPDATE messages SET startup=2 WHERE id='reply'",
    ] {
        assert!(store.connection.execute(sql, []).is_err(), "{sql}");
        check(&store);
    }
    // Simulate stale, well-formed derived metadata; recovery reconstructs it
    // from the actual mailbox before making agents runnable.
    store
        .connection
        .execute("UPDATE agents SET head_startup=0 WHERE id='a'", [])
        .unwrap();
    store.recover_agents().unwrap();
    check(&store);
}

#[derive(Clone, Copy)]
enum Mutation {
    InsertMessage,
    DeleteMessage,
    SetMessageState,
    SetStartup,
    MoveMessage,
    SetMode,
    SetAgentState,
    SetRedirect,
    SetRecipientState,
}
impl Mutation {
    fn from_random(random: u64) -> Self {
        const CASES: [Mutation; 9] = [
            Mutation::InsertMessage,
            Mutation::DeleteMessage,
            Mutation::SetMessageState,
            Mutation::SetStartup,
            Mutation::MoveMessage,
            Mutation::SetMode,
            Mutation::SetAgentState,
            Mutation::SetRedirect,
            Mutation::SetRecipientState,
        ];
        CASES[usize::try_from((random / 2880) % 9).unwrap()]
    }
}

fn execute(store: &Store, sql: &str, parameters: impl rusqlite::Params) {
    store.connection.execute(sql, parameters).unwrap();
}

fn mutate(store: &Store, random: u64) {
    let message = format!("m{}", random % 80);
    let recipient = ["a", "b", "c"][usize::try_from((random / 80) % 3).unwrap()];
    let state = [
        "accepted",
        "delivered_to_turn",
        "pending_after_failed_turn",
        "undeliverable",
        "accepted",
        "stale_turn",
    ][usize::try_from((random / 240) % 6).unwrap()];
    let startup = i64::try_from((random / 1440) % 2).unwrap();
    let mode = if random & 0x4000 == 0 {
        "next-turn"
    } else {
        "steer"
    };
    let redirect = i64::from(random & 0x10000 != 0);
    match Mutation::from_random(random) {
        Mutation::InsertMessage => {
            execute(
                store,
                concat!(
                    "INSERT OR IGNORE INTO messages(id,sender,recipient,body,state,startup,mode,redirect) ",
                    "VALUES(?1,'fixture',?2,'text',?3,?4,?5,?6)",
                ),
                params![message, recipient, state, startup, mode, redirect],
            );
        }
        Mutation::DeleteMessage => {
            execute(store, "DELETE FROM messages WHERE id=?1", [&message]);
        }
        Mutation::SetMessageState => {
            execute(
                store,
                "UPDATE messages SET state=?2 WHERE id=?1",
                params![message, state],
            );
        }
        Mutation::SetStartup => {
            execute(
                store,
                "UPDATE messages SET startup=?2 WHERE id=?1",
                params![message, startup],
            );
        }
        Mutation::MoveMessage => {
            execute(
                store,
                "UPDATE messages SET recipient=?2,state=?3,startup=?4,mode=?5,redirect=?6 WHERE id=?1",
                params![message, recipient, state, startup, mode, redirect],
            );
        }
        Mutation::SetMode => {
            execute(
                store,
                "UPDATE messages SET mode=?2 WHERE id=?1",
                params![message, mode],
            );
        }
        Mutation::SetAgentState => mutate_agent(store, random, recipient, startup),
        Mutation::SetRedirect => {
            execute(
                store,
                "UPDATE messages SET redirect=?2 WHERE id=?1",
                params![message, redirect],
            );
        }
        Mutation::SetRecipientState => {
            execute(
                store,
                "UPDATE messages SET state=?1 WHERE recipient=?2",
                params![state, recipient],
            );
        }
    }
}

fn mutate_agent(store: &Store, random: u64, recipient: &str, startup: i64) {
    execute(
        store,
        concat!(
            "UPDATE agents SET state=?2,task_outcome=CASE WHEN ended_at IS NULL THEN ?3 ELSE ",
            "task_outcome END,expired=max(expired,?4),paused=?4,ended_at=CASE WHEN ended_at IS ",
            "NULL AND (?3 IN ('completed','cancelled') OR ?4=1) THEN 1 ELSE ended_at END WHERE ",
            "id=?1",
        ),
        params![
            recipient,
            if startup == 0 { "stopped" } else { "running" },
            if random & 0x8000 == 0 {
                "open"
            } else {
                "completed"
            },
            startup
        ],
    );
}
