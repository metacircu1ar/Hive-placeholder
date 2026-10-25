//! SQL statements for login availability and auth continuation records.
pub(super) const AUTH_GENERATIONS: &str = r"
SELECT generation
FROM login_availability
WHERE profile = ?1
ORDER BY generation
";

pub(super) const DELIVERABLE_INPUT: &str = r"
SELECT pending_count > 0
FROM agents
WHERE id = ?1
";

pub(super) const RESTORATION_PROFILES: &str = r"
SELECT DISTINCT profile
FROM login_availability
WHERE state = 'unavailable'
    OR reason IN ('authentication', 'usage_and_authentication')
    OR probe_error IS NOT NULL
ORDER BY profile
";

pub(super) const CUTOFF_INPUT: &str = r"
SELECT m.id
FROM messages m
JOIN deliveries d
ON d.message_id = m.id
WHERE m.recipient = ?1
    AND d.execution = ?2
    AND d.recipient_turn = ?3
    AND m.schedule_integrity IS NULL
    AND m.mode = 'next-turn'
    AND m.state IN ('submitting', 'submitted', 'uncertain', 'failed')
LIMIT 1
";

pub(super) const HANDOFF_EVENTS: &str = r"
SELECT data
FROM events
WHERE agent = ?1
    AND kind IN ('assistant_message', 'turn_finished')
ORDER BY seq DESC
LIMIT 12
";

pub(super) const HANDOFF_INPUTS: &str = r"
SELECT json_object('id', id, 'sender', sender, 'state', state, 'body', body)
FROM messages
WHERE recipient = ?1
    AND state != 'scheduled'
    AND kind IS NOT 'continuation'
ORDER BY seq DESC
LIMIT 12
";

pub(super) const HANDOFF_ORIGINAL_TASK: &str = r"
SELECT body
FROM messages
WHERE recipient = ?1
    AND startup = 1
ORDER BY seq
LIMIT 1
";

pub(super) const HANDOFF_CHILDREN: &str = r"
SELECT json_object('id', id, 'state', state, 'task_outcome', task_outcome)
FROM agents
WHERE parent_id = ?1
ORDER BY listing_seq DESC
LIMIT 32
";

pub(super) const HANDOFF_OUTGOING_MESSAGES: &str = "\nSELECT json_object('id', id, \
    'recipient', recipient, 'state', state, 'not_before', not_before)\nFROM messages\nWHERE \
    sender = ?1\n    AND state IN ('scheduled', 'accepted', 'submitting', 'submitted')\nORDER \
    BY seq DESC\nLIMIT 16\n";

pub(super) const HANDOFF_PENDING_APPROVAL: &str = r"
SELECT json_object('id', id, 'title', title, 'message', message)
FROM approvals
WHERE agent = ?1
    AND state = 'pending'
LIMIT 1
";

pub(super) const AUTH_REVISION: &str = r"
SELECT coalesce(max(revision), 0)
FROM login_availability
";

pub(super) const AVAILABILITY: &str = "\nSELECT json_object('profile', profile, 'generation', \
generation, 'revision', revision, 'key', json(key_authority), 'state',\n        state, \
'until', until, 'retry_at', retry_at, 'probe_id', probe_id, 'probe_agent', probe_agent,\n     \
\x20  'probe_message', probe_message, 'probe_deadline', probe_deadline, 'probe_error', \
probe_error,\n        'window', window, 'reason', reason, 'confirmed_at', confirmed_at, \
'updated_at', updated_at)\nFROM login_availability\nWHERE profile = ?1\n    AND generation = \
?2\n";

pub(super) const CONTINUATION_ORIGIN: &str = r"
SELECT kind, continuation_root, continuation_attempt
FROM messages
WHERE recipient = ?1
    AND id = ?2
    AND schedule_integrity IS NULL
";

pub(super) const INSERT_CONTINUATION: &str = "\nINSERT OR IGNORE INTO messages(id, sender, \
    recipient, body, state, redirect, kind, continuation_root,\n        \
    continuation_attempt)\nVALUES (?1, 'hive', ?2, ?3, 'accepted', 1, 'continuation', ?4, \
    ?5)\n";

pub(super) const INSERT_CONTINUATION_DELIVERY: &str = r"
INSERT INTO deliveries(message_id)
VALUES (?1)
";

pub(super) const CONTINUATION_MATCHES: &str = r"
SELECT EXISTS (
    SELECT 1
    FROM messages
    WHERE id = ?1
        AND sender = 'hive'
        AND recipient = ?2
        AND kind = 'continuation'
        AND continuation_root = ?3
        AND continuation_attempt = ?4
        AND schedule_integrity IS NULL)
";

pub(super) const AFFECTED_AUTH_AGENTS: &str = concat!(
    r"
SELECT a.id
FROM agents a
JOIN login_availability l
ON l.profile = a.auth
    AND l.generation = json_extract(a.data, '$.auth_generation')
WHERE a.branch_hold = 0
    AND (a.branch_gate IS NULL
        OR a.branch_gate = 'bound')
    AND a.auth_group IS NOT NULL
    AND a.waiting = 0
    AND a.expired = 0
    AND a.task_outcome = 'open'
    AND (l.state != 'available'
        OR l.probe_error IS NOT NULL)
    AND (a.pending_count > 0
        OR a.auth_group NOT IN (
        SELECT value
        FROM json_each(?1)))
",
    crate::runtime::features::executable!("a.comb_bound"),
    r"ORDER BY a.id
LIMIT 128
"
);

pub(super) const AUTH_SWITCHES_AFTER: &str = concat!(
    r"
SELECT id
FROM agents
WHERE branch_hold = 0
    AND (branch_gate IS NULL
        OR branch_gate = 'bound')
    AND id > ?1
    AND waiting = 1
    AND waiting_reason = 'switch'
    AND claimed = 0
    AND blocked = 0
    AND expired = 0
    AND task_outcome = 'open'
    AND json_extract(data, '$.recovery_fence') IS NULL
",
    crate::runtime::features::executable!("comb_bound"),
    r"ORDER BY id
LIMIT 128
"
);

pub(super) const NEXT_LOGIN_DEADLINE: &str = concat!(
    r"
SELECT min(deadline)
FROM (
    SELECT coalesce(until, retry_at) AS deadline
    FROM login_availability
    WHERE state = 'limited'
        AND probe_error IS NULL
    UNION ALL
    SELECT l.probe_deadline
    FROM login_availability l
    LEFT JOIN agents a ON a.id = l.probe_agent
    WHERE l.probe_id IS NOT NULL
        AND l.probe_error IS NULL
        AND coalesce(a.claimed, 0) = 0",
    r")
"
);

pub(super) const NEXT_BRANCH_DEADLINE: &str = concat!(
    r"
SELECT min(deadline)
FROM branches
WHERE holding = 1
    AND deadline IS NOT NULL",
    crate::runtime::features::executable!(
        "coalesce((SELECT comb_bound FROM agents WHERE id = branches.agent), 0)"
    )
);

pub(super) const EXPIRED_LOGIN_WINDOWS: &str = r"
SELECT profile, generation
FROM login_availability
WHERE state = 'limited'
    AND probe_error IS NULL
    AND coalesce(until, retry_at) <= ?1
ORDER BY coalesce(until, retry_at), profile
LIMIT 128
";

pub(super) const QUOTA_WAITERS: &str = concat!(
    "\nSELECT a.id\nFROM agents a\nWHERE a.waiting = 1\n    AND \
        a.waiting_reason = 'quota'\n    AND a.expired = 0\n    AND a.task_outcome \
        = 'open'\n    AND (a.quota_wait_until <= ?1\n        OR EXISTS (\n        \
        SELECT 1\n        FROM login_availability l\n        WHERE l.revision > \
        a.auth_checked_revision\n            AND (l.profile = a.auth\n                \
        OR l.profile = coalesce(json_extract(a.data, \
        '$.auth_switch_pending.target'),\n                    a.auth_group)\n                \
        OR l.profile IN (\n                SELECT m.value\n                FROM \
        json_each(?2) g, json_each(g.value) m\n                WHERE g.key = \
        coalesce(json_extract(a.data, '$.auth_switch_pending.target'), \
        a.auth_group)))))\n",
    crate::runtime::features::executable!("a.comb_bound"),
    r"ORDER BY a.id
LIMIT 128
"
);

pub(super) const SAVE_AVAILABILITY: &str = "\nINSERT INTO login_availability(profile, \
    generation, revision, state, until, retry_at, probe_id,\n        probe_agent, \
    probe_message, probe_deadline, probe_error, window, reason, confirmed_at,\n        \
    updated_at, key_authority)\nVALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, \
    ?13, ?14, ?15, ?16)\nON CONFLICT (profile, generation) DO UPDATE\nSET revision = \
    excluded.revision, state = excluded.state, until = excluded.until, retry_at = \
    excluded.retry_at,\n    probe_id = excluded.probe_id, probe_agent = excluded.probe_agent, \
    probe_message = excluded.probe_message,\n    probe_deadline = excluded.probe_deadline, \
    probe_error = excluded.probe_error, window = excluded.window,\n    reason = \
    excluded.reason, confirmed_at = excluded.confirmed_at, updated_at = \
    excluded.updated_at,\n    key_authority = excluded.key_authority\n";

pub(super) const PROBE_INPUT: &str = r"
SELECT state
FROM messages
WHERE recipient = ?1
    AND id = ?2
    AND mode = 'next-turn'
";

pub(super) const PROBE_CANDIDATES: &str = concat!(
    r"
SELECT l.profile, l.generation
FROM login_availability l
LEFT JOIN agents a
ON a.id = l.probe_agent
LEFT JOIN messages m
ON m.id = l.probe_message
    AND m.recipient = l.probe_agent
WHERE l.probe_id IS NOT NULL
    AND l.probe_error IS NULL
    AND coalesce(a.claimed, 0) = 0
    AND (l.probe_deadline <= ?1
        OR a.id IS NULL
        OR m.id IS NULL
        OR m.state IN ('accepted', 'completed', 'failed', 'interrupted', 'undeliverable')
        OR json_extract(a.data, '$.conversation_clean') = 1)
",
    r"ORDER BY l.probe_deadline, l.profile
LIMIT 128
"
);

pub(super) const CUTOFF_AGENTS: &str = r"
SELECT id
FROM agents
WHERE id > ?1
    AND json_extract(data, '$.auth_cutoff_turn') IS NOT NULL
ORDER BY id
LIMIT 128
";
