//! SQL statements for branch intents, source holds and fork completion.
pub(super) const HANDOFF_INPUTS: &str = "\nSELECT json_object('turn', d.recipient_turn, \
    'sender', m.sender, 'body', substr(m.body, 1, 4096))\nFROM messages m\nJOIN deliveries \
    d\nON d.message_id = m.id\nWHERE m.recipient = ?1\n    AND d.thread = ?2\n    AND \
    d.recipient_turn IS NOT NULL\n    AND m.state IN ('completed', 'failed', 'interrupted')\n    \
    AND m.mode = 'next-turn'\nORDER BY m.seq DESC\nLIMIT 12\n";

pub(super) const HANDOFF_EVENTS: &str = r"
SELECT e.data
FROM events e
WHERE e.agent = ?1
    AND e.kind = 'assistant_message'
    AND EXISTS (
    SELECT 1
    FROM deliveries d
    JOIN messages m
    ON m.id = d.message_id
    WHERE m.recipient = e.agent
        AND d.thread = ?2
        AND d.execution = e.execution
        AND d.recipient_turn = json_extract(e.data, '$.turn_id'))
ORDER BY e.seq DESC
LIMIT 12
";

pub(super) const BRANCH_DEPENDENTS: &str = r"
SELECT agent
FROM branches
WHERE source = ?1
    AND holding = 1
ORDER BY agent
LIMIT 20
";

pub(super) const BRANCH_INTENT: &str = "\nSELECT source, source_thread, source_execution, \
    source_turn, phase, deadline, source_outcome,\n    source_handoff, native_thread, \
    error\nFROM branches\nWHERE agent = ?1\n";

pub(super) const BOUNDARY_UNACKNOWLEDGED_INPUT: &str = r"
SELECT EXISTS (
    SELECT 1
    FROM messages
    WHERE recipient = ?1
        AND mode = 'next-turn'
        AND state IN ('submitting', 'submitted')
        AND id NOT IN (
        SELECT message_id
        FROM deliveries
        WHERE recipient_turn IS NOT NULL))
";

pub(super) const LATEST_ATTEMPT_RECEIPT: &str = r"
SELECT m.state, d.thread, d.recipient_turn, d.execution
FROM messages m
JOIN deliveries d
ON d.message_id = m.id
WHERE m.recipient = ?1
    AND m.id = ?2
";

pub(super) const BRANCHES_TO_SETTLE: &str = concat!(
    r"
SELECT agent
FROM (
    SELECT b.agent
    FROM branches b
    JOIN agents s
    ON s.id = b.source
    WHERE b.phase = 'waiting_for_source'
        AND (b.deadline <= ?1
            OR (s.turn_id IS NULL
                AND (b.deadline IS NULL
                    OR s.claimed = 0)))
    UNION ALL
    SELECT agent
    FROM branches
    WHERE holding = 1
        AND deadline <= ?1
        AND phase != 'waiting_for_source'
    UNION ALL
    SELECT b.agent
    FROM branches b
    JOIN agents a
    ON a.id = b.agent
    WHERE b.phase = 'forking'
        AND a.claimed = 0
        AND (b.deadline IS NULL
            OR b.deadline > ?1))
WHERE 1 = 1
",
    crate::runtime::features::executable!(
        "coalesce((SELECT comb_bound FROM agents WHERE id = agent), 0)"
    ),
    r"
LIMIT 128
"
);

pub(super) const SETTLE_BRANCH: &str = r"
UPDATE branches
SET deadline = ?2, source_outcome = ?3, source_handoff = ?4, phase = ?5
WHERE agent = ?1
";

pub(super) const FAIL_BRANCH: &str = r"
UPDATE branches
SET phase = ?2, error = ?3, holding = ?4
WHERE agent = ?1
";

pub(super) const RELEASE_BRANCH_HOLD: &str = r"
UPDATE branches
SET holding = 0
WHERE agent = ?1
";

pub(super) const BEGIN_BRANCH: &str = r"
UPDATE branches
SET phase = 'forking'
WHERE agent = ?1
";

pub(super) const REJECT_BRANCH_CREATION: &str = r"
UPDATE branches
SET phase = 'ready_to_fork', error = coalesce(error, ?2)
WHERE agent = ?1
";

pub(super) const RECORD_BRANCH_NATIVE: &str = r"
UPDATE branches
SET native_thread = ?2
WHERE agent = ?1
    AND source_thread != ?2
    AND (native_thread IS NULL
        OR native_thread = ?2)
";

pub(super) const BIND_BRANCH: &str = r"
UPDATE branches
SET phase = 'bound', holding = 0, native_thread = ?2, error = NULL
WHERE agent = ?1
";

pub(super) const INSTALL_BRANCH: &str = "\nINSERT INTO branches(agent, source, source_thread, \
    source_execution, source_turn, phase, deadline,\n        source_outcome)\nVALUES (?1, ?2, \
    ?3, ?4, ?5, ?6, ?7, ?8)\n";

pub(super) const INHERIT_BRANCH_PRIORITY: &str = r"
UPDATE agents
SET branch_priority = (
    SELECT CASE
    WHEN coordinator = 1 THEN 0
    ELSE 1
    END
    FROM agents
    WHERE id = ?2)
WHERE id = ?1
";
