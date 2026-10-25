//! Runtime messaging and recovery SQL.

pub(super) const ACTIVE_AGENT_COUNT: &str = r"
SELECT active
FROM runtime_counts
WHERE id = 1
";

pub(super) const NEXT_READY_AGENT: &str = concat!(
    r"
SELECT id
FROM agents
WHERE runnable = 1
    AND admission_class = ?1
",
    crate::runtime::features::executable!("comb_bound"),
    r"ORDER BY pending_seq, id
LIMIT 1
"
);

pub(super) const CLAIM_AGENT: &str = r"
UPDATE agents
SET claimed = 1
WHERE id = ?1
    AND runnable = 1
";

pub(super) const RELEASE_AGENT_CLAIM: &str = r"
UPDATE agents
SET claimed = 0
WHERE id = ?1
RETURNING branch_hold, branch_gate
";

pub(super) const HAS_READY_AGENT: &str = concat!(
    r"
SELECT EXISTS (
    SELECT 1
    FROM agents
    WHERE runnable = 1",
    crate::runtime::features::executable!("comb_bound"),
    r")
"
);

pub(super) const AGENT_PRIORITY: &str = r"
SELECT priority
FROM agents
WHERE id = ?1
";

pub(super) const CHECKPOINT_PASSIVE: &str = r"
PRAGMA wal_checkpoint(PASSIVE)
";

pub(super) const CHECKPOINT_TRUNCATE: &str = r"
PRAGMA wal_checkpoint(TRUNCATE)
";

pub(super) const PROVIDER_AGENT_COUNT: &str = r"
SELECT count(*)
FROM agents
WHERE provider_id = ?1
    AND json_extract(data, '$.conversation_clean') IS NOT 1
";

pub(super) const INSERT_PROVIDER_EVENT: &str = r"
INSERT INTO provider_events(provider, at, kind, data)
VALUES (?1, ?2, ?3, ?4)
";

pub(super) const INSERT_INSTRUCTION_SNAPSHOT: &str = r"
INSERT OR IGNORE INTO instruction_snapshots(body)
VALUES (?1)
";

pub(super) const BIND_AGENT_INSTRUCTIONS: &str = r"
INSERT OR IGNORE INTO agent_instructions
SELECT ?1, id
FROM instruction_snapshots
WHERE body = ?2
";

// The `agent_records` projection of one agent, without its `json_set`. The
// columns after `data` follow `agents::RECORD_FIELDS`.
pub(super) const AGENT_ROW: &str = "\nSELECT data, branch_hold, branch_gate, ended_at, \
    parent_id, auth, auth_group,\n    auth_checked_revision, waiting_reason, \
    quota_wait_until, lifetime, state, task_outcome,\n    paused, expired, thread_id, \
    execution_id, turn_id, control, completion_pending, result,\n    error, provider_id, \
    provider_pid, provider_spawn_before, provider_spawn_after\nFROM agents\nWHERE id = ?1\n";

pub(super) const BIND_DELIVERY_TURN: &str = r"
UPDATE deliveries
SET recipient_turn = ?1, execution = ?2, provider = ?3, thread = ?4
WHERE message_id = ?5
";

pub(super) const INSERT_ARTIFACT: &str = r"
INSERT INTO artifacts(id, bytes, created_at, agent, join_id)
VALUES (?1, ?2, ?3, ?4, ?5)
";

pub(super) const INSERT_EVENT: &str = r"
INSERT INTO events(agent, execution, at, kind, data)
VALUES (?1, ?2, ?3, ?4, ?5)
";

pub(super) const EVENT_PAGE: &str = r"
SELECT seq, execution, at, kind, data
FROM events
WHERE agent = ?1
    AND seq > ?2
ORDER BY seq
LIMIT ?3
";

// Newest retained events before a cursor, for paging history backwards. Agent
// sequences are sparse, so a forward cursor cannot be derived from a count.
pub(super) const EVENT_PAGE_BEFORE: &str = r"
SELECT seq, execution, at, kind, data
FROM events
WHERE agent = ?1
    AND seq < ?2
    AND seq > ?3
ORDER BY seq DESC
LIMIT ?4
";

pub(super) const PRUNED_EVENT_CURSOR: &str = r"
SELECT pruned_through
FROM event_retention
WHERE agent = ?1
";

pub(super) const INSERT_MESSAGE: &str = r"
INSERT INTO messages(id, sender, recipient, body, state, startup, redirect)
VALUES (?1, ?2, ?3, ?4, 'accepted', ?5, ?6)
";

pub(super) const INSERT_DELIVERY: &str = r"
INSERT INTO deliveries(message_id, sender_turn)
VALUES (?1, (
        SELECT turn_id
        FROM agents
        WHERE id = ?2))
";

pub(super) const SET_MESSAGE_STATE: &str = r"
UPDATE messages
SET state = ?1
WHERE id = ?2
    AND sender = ?3
";

pub(super) const DEFER_SUBMISSION: &str = r"
UPDATE messages
SET state = 'accepted'
WHERE recipient = ?1
    AND state = 'submitting'
    AND mode = 'next-turn'
";

pub(super) const REJECT_SUBMISSION: &str = r"
UPDATE messages
SET state = 'failed'
WHERE recipient = ?1
    AND state = 'submitting'
    AND mode = 'next-turn'
";

pub(super) const MARK_MESSAGES_UNCERTAIN: &str = r"
UPDATE messages
SET state = 'uncertain'
WHERE recipient = ?1
    AND state IN ('submitting', 'submitted')
";

pub(super) const MARK_MESSAGES_UNDELIVERABLE: &str = r"
UPDATE messages
SET state = 'undeliverable'
WHERE recipient = ?1
    AND state = 'accepted'
";

pub(super) const PRIOR_REQUEST: &str = r"
SELECT body, response, protocol
FROM requests
WHERE sender = ?1
    AND id = ?2
";

pub(super) const REMEMBER_REQUEST: &str = r"
INSERT INTO requests
VALUES (?1, ?2, ?3, ?4, ?5)
";

pub(super) const RECOVER_AGENTS: &str = "\nUPDATE redirect_interrupts\nSET state = \
    'target_lost'\nWHERE state IN ('pending', 'sent');\nUPDATE messages\nSET state = \
    'stale_turn'\nWHERE mode = 'steer'\n    AND state = 'accepted';\nUPDATE messages\nSET \
    state = 'uncertain'\nWHERE state IN ('submitting', 'submitted');\nUPDATE agents\nSET \
    state = 'stopped', turn_id = NULL, control = NULL, completion_pending = NULL, claimed = \
    0,\n    blocked = 0, data = json_set(json_remove(data, '$.steering_supported'), \
    '$.turn_selection',\n        NULL, '$.redirect_pending', json('false'));\n";

pub(super) const PENDING_CONTROLS: &str = r"
SELECT id
FROM agents
WHERE claimed = 0
    AND (control = 'stop'
        OR completion_pending IS NOT NULL)
ORDER BY id
LIMIT 128
";

pub(super) const AGENT_INSTRUCTIONS: &str = r"
SELECT body
FROM instruction_snapshots s
JOIN agent_instructions a
ON a.snapshot = s.id
WHERE a.agent = ?1
";

pub(super) const DEPENDENT_CHILDREN: &str = r"
SELECT a.id, v.data
FROM agents a
JOIN agent_records v
ON v.id = a.id
WHERE a.parent_id = ?1
    AND a.lifetime = 'with_parent'
    AND a.expired = 0
ORDER BY a.listing_seq
LIMIT ?2
";

// An alias must never choose a sibling merely because another child ended.
// The parent index bounds this lookup even in a million-agent state directory.
pub(super) const RELATIVE_CHILDREN: &str = r"
SELECT id FROM agents WHERE parent_id = ?1 LIMIT 2
";

pub(super) const AGENT_PAGE: &str = r"
SELECT a.listing_seq, v.data, (
    SELECT id
    FROM approvals
    WHERE agent = a.id
        AND state = 'pending')
FROM agents a
JOIN agent_records v
ON v.id = a.id
WHERE a.listing_seq > ?1
    AND (?2 IS NULL
        OR a.parent_id = ?2)
    AND (?4 IS NULL
        OR a.auth = ?4
        OR a.auth_group = ?4)
ORDER BY a.listing_seq
LIMIT ?3
";

pub(super) const PARENT_FILTER_EXISTS: &str = r"
SELECT EXISTS (SELECT 1 FROM agents WHERE id = ?1)
    OR EXISTS (SELECT 1 FROM purged_agents WHERE id = ?1)
";

pub(super) const AUTH_FILTER_EXISTS: &str = r"
SELECT EXISTS (SELECT 1 FROM agents WHERE auth = ?1 OR auth_group = ?1)
";

pub(super) const NEXT_MESSAGE: &str = r"
SELECT id, sender, recipient, body, state, join_id IS NOT NULL, has_attachments
FROM messages
WHERE recipient = ?1
    AND state = 'accepted'
    AND mode = 'next-turn'
ORDER BY startup DESC, redirect DESC, seq
LIMIT 1
";

pub(super) const MESSAGE_PAGE: &str = "\nSELECT m.seq, m.id, m.sender, m.recipient, m.body, \
    m.state, CASE\nWHEN m.redirect = 1\n    AND m.kind IS NULL THEN 'redirect'\nELSE \
    m.mode\nEND, d.execution, d.recipient_turn, d.error, m.kind, m.not_before, \
    m.schedule_integrity, m.has_attachments\nFROM messages m\nLEFT JOIN deliveries d\nON \
    d.message_id = m.id\nWHERE (m.recipient = ?1\n        OR m.sender = ?1)\n    AND m.seq < \
    ?2\nORDER BY m.seq DESC\nLIMIT ?3\n";

pub(super) const ARTIFACT_BYTES: &str = r"
SELECT bytes
FROM artifacts
WHERE id = ?1
";

pub(super) const CREATE_COMPACTION_FRONTIER: &str = r"
CREATE TEMP TABLE compact_frontier(agent TEXT PRIMARY KEY, through INTEGER);
";

pub(super) const DELETE_EVENT: &str = r"
DELETE FROM events
WHERE seq = ?1
";

pub(super) const FINISH_COMPACTION: &str = r"
DROP TABLE compact_frontier;
PRAGMA wal_checkpoint(TRUNCATE);
VACUUM;
";

pub(super) const ARTIFACT_MANIFEST: &str = r"
SELECT id, bytes, created_at
FROM artifacts
ORDER BY id
";

pub(super) const PRUNABLE_EVENTS: &str = r"
SELECT e.seq, e.agent
FROM events e
JOIN compact_frontier c
ON c.agent = e.agent
WHERE e.seq > ?1
    AND e.seq <= c.through
    AND e.at < ?2
ORDER BY e.seq
LIMIT 1024
";

pub(super) const ADVANCE_EVENT_RETENTION: &str = r"
INSERT INTO event_retention
VALUES (?1, ?2)
ON CONFLICT (agent) DO UPDATE
SET pruned_through = max(pruned_through, excluded.pruned_through)
";

pub(super) static SAVE_AGENT: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    let columns = super::schema::HOT;
    let marks = vec!["?"; columns.len() + 5].join(",");
    let updates = columns
        .iter()
        .map(|n| format!("{n}=excluded.{n}"))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "\nINSERT INTO agents(id, data, {}, ended_at, process_options, \
            reserved_process_options)\nVALUES ({marks})\nON CONFLICT (id) DO \
            UPDATE\nSET data = excluded.data, {updates}, ended_at = \
            coalesce(agents.ended_at, excluded.ended_at),\n    process_options = \
            excluded.process_options,\n    reserved_process_options = \
            excluded.reserved_process_options\n",
        columns.join(",")
    )
});

pub(super) fn begin_savepoint(depth: usize) -> String {
    format!(
        r"
SAVEPOINT tx_{depth}
"
    )
}

pub(super) fn release_savepoint(depth: usize) -> String {
    format!(
        r"
RELEASE tx_{depth}
"
    )
}

pub(super) fn rollback_savepoint(depth: usize) -> String {
    format!(
        r"
ROLLBACK TO tx_{depth};
RELEASE tx_{depth}
"
    )
}

// CASE is lazy: polling active or ended agents must not walk their event suffix.
pub const AGENT_IDLE_SINCE: &str = "\nSELECT CASE\nWHEN ended_at IS NOT NULL THEN \
    ended_at\nWHEN state = 'running'\n    OR turn_id IS NOT NULL\n    OR EXISTS (\n    SELECT \
    1\n    FROM messages\n    WHERE recipient = a.id\n        AND mode = 'next-turn'\n        \
    AND state IN ('submitting', 'submitted')) THEN NULL\nELSE (\n    SELECT at\n    FROM \
    events\n    WHERE agent = a.id\n        AND kind IN ('turn_finished', 'turn_started', \
    'message_accepted', 'redirect_accepted',\n            'task_completed', 'spawned')\n    \
    ORDER BY seq DESC\n    LIMIT 1)\nEND\nFROM agents a\nWHERE id = ?1\n";

// Narrow scalars only; do not deserialize full agent records or walk descendants per root.
pub const STALLED_AGENTS: &str = "\nSELECT a.id, a.parent_id, json_extract(a.data, '$.role'), \
    a.paused, (a.state = 'running'\n        OR a.turn_id IS NOT NULL\n        OR EXISTS (\n        \
    SELECT 1\n        FROM messages\n        WHERE recipient = a.id\n            AND mode = \
    'next-turn'\n            AND state IN ('submitting', 'submitted'))), (a.pending_count > \
    0\n        OR a.waiting = 1\n        OR EXISTS (SELECT 1 FROM joins WHERE requester = \
    a.id AND state IN ('collecting', 'deadline_passed'))\n        OR a.branch_hold > 0\n        \
    OR (a.branch_gate IS NOT NULL\n            AND a.branch_gate != 'bound')\n        OR \
    EXISTS (\n        SELECT 1\n        FROM approvals\n        WHERE agent = a.id\n            \
    AND state = 'pending')\n        OR EXISTS (\n        SELECT 1\n        FROM messages\n        \
    WHERE recipient = a.id\n            AND state = 'scheduled')\n        OR EXISTS (\n        \
    SELECT 1\n        FROM messages\n        WHERE recipient = a.id\n            AND mode = \
    'steer'\n            AND state = 'accepted')), json_extract(a.data, '$.name')\nFROM \
    agents a\nWHERE a.task_outcome = 'open'\n    AND a.expired = 0\n";

pub(super) const FENCE_PROVIDER: &str = r"
UPDATE agents
SET blocked = 1, data = json_set(data, '$.recovery_fence', 'execution_cleanup_uncertain')
WHERE provider_id = ?1
    AND json_extract(data, '$.conversation_clean') IS NOT 1
";

pub(super) const UNBLOCK_AGENT: &str = r"
UPDATE agents
SET blocked = 0
WHERE id = ?1
";

pub(super) const BIND_CALL_START: &str = r"
UPDATE call_results
SET start_seq = last_insert_rowid()
WHERE message_id = ?1
";

pub(super) const RESERVATION_ACCEPTED: &str = r"
SELECT EXISTS (
    SELECT 1
    FROM messages
    WHERE recipient = ?1
        AND id = ?2
        AND state = 'accepted'
        AND mode = 'next-turn')
";

use super::owned::{CountField, OWNED_TABLES, OwnedTable, Ownership};

impl OwnedTable {
    fn scope(&self, counting: bool) -> String {
        match self.owner {
            Ownership::Column(column) => format!("{column} IN purge_scope"),
            Ownership::Incoming => {
                "message_id IN (SELECT id FROM messages WHERE recipient IN purge_scope)".into()
            }
            Ownership::Calls => {
                "message_id IN (SELECT id FROM messages WHERE recipient IN purge_scope \
                UNION SELECT message_id FROM join_members WHERE requester IN purge_scope)"
                    .into()
            }
            Ownership::Snapshots => {
                let surviving = if counting {
                    " AND m.agent NOT IN purge_scope"
                } else {
                    ""
                };
                format!(
                    "id IN purge_snapshots AND NOT EXISTS(SELECT 1 FROM agent_instructions m \
                        WHERE m.snapshot=instruction_snapshots.id{surviving})"
                )
            }
        }
    }
    fn single(&self) -> String {
        match self.owner {
            Ownership::Column(column) => format!("{column}=?1"),
            Ownership::Incoming => {
                "message_id IN (SELECT id FROM messages WHERE recipient=?1)".into()
            }
            Ownership::Calls => {
                "message_id IN (SELECT id FROM messages WHERE recipient=?1 UNION SELECT \
                message_id FROM join_members WHERE requester=?1)"
                    .into()
            }
            Ownership::Snapshots => unreachable!("shared snapshots are excluded from GC work"),
        }
    }
}

pub(super) struct OwnedCount {
    pub sql: String,
    pub destination: CountField,
}
pub(super) static PURGE_COUNTS: std::sync::LazyLock<Vec<OwnedCount>> =
    std::sync::LazyLock::new(|| {
        OWNED_TABLES
            .iter()
            .filter_map(|table| table.count.map(|destination| (table, destination)))
            .map(|(table, destination)| OwnedCount {
                sql: format!(
                    r"
SELECT count(*)
FROM {}
WHERE {}
",
                    table.table,
                    table.scope(true)
                ),
                destination,
            })
            .collect()
    });

#[expect(
    clippy::expect_used,
    reason = "Writing formatted SQL into a String cannot fail."
)]
pub(super) static DELETE_PURGE_SCOPE: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| {
        use std::fmt::Write as _;
        let mut sql = r"
INSERT INTO purged_agents
SELECT id, parent_id
FROM agents
WHERE id IN purge_scope;
INSERT INTO artifact_deletions
SELECT id
FROM artifacts
WHERE agent IN purge_scope;
"
        .to_owned();
        for table in OWNED_TABLES {
            write!(
                sql,
                r"
DELETE FROM {}
WHERE {};
",
                table.table,
                table.scope(false)
            )
            .expect("writing SQL to a String cannot fail");
        }
        sql
    });

pub(super) static GC_OWNED_ROWS: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        r"
SELECT 1 + {}
",
        OWNED_TABLES
            .iter()
            .filter(|table| table.gc)
            .map(|table| format!(
                "(SELECT count(*) FROM {} WHERE {})",
                table.table,
                table.single()
            ))
            .collect::<Vec<_>>()
            .join(" +\n            ")
    )
});

pub(super) const SNAPSHOT: &str = r"
BEGIN
";

pub(super) const SNAPSHOT_2: &str = r"
ROLLBACK
";

pub(super) const AGENT_CLAIMED: &str = "SELECT claimed FROM agents WHERE id = ?1";
pub(super) const FILL_SINGLE_SCOPE: &str = "INSERT INTO purge_scope VALUES (?1)";
