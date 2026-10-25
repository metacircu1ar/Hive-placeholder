//! Scheduled SQL statements and query builders.

pub(in crate::runtime::store) const DUE: &str = concat!(
    r"
SELECT seq, id, sender, recipient, body, not_before, join_id, has_attachments
FROM messages
WHERE state = 'scheduled'
    AND not_before <= ?1
",
    crate::runtime::features::executable!(
        "coalesce((SELECT comb_bound FROM agents WHERE id = messages.recipient), 0)"
    ),
    r"ORDER BY not_before, seq
LIMIT 128
"
);

pub(in crate::runtime::store) const CANCEL: &str = r"
SELECT seq, id, sender, recipient, body, not_before, join_id, has_attachments
FROM messages
WHERE recipient = ?1
    AND state = 'scheduled'
    AND mode = 'next-turn'
ORDER BY seq
LIMIT 128
";

pub(in crate::runtime::store) const NEXT: &str = concat!(
    r"
SELECT min(not_before)
FROM messages
WHERE state = 'scheduled'
",
    crate::runtime::features::executable!(
        "coalesce((SELECT comb_bound FROM agents WHERE id = messages.recipient), 0)"
    )
);

pub(super) const ENQUEUE_SCHEDULED: &str = r"
INSERT INTO messages(id, sender, recipient, body, state, not_before)
VALUES (?1, ?2, ?3, ?4, 'scheduled', ?5)
";

pub(super) const ENQUEUE_SCHEDULED_2: &str = r"
INSERT INTO deliveries(message_id, sender_turn, status)
VALUES (?1, (
        SELECT turn_id
        FROM agents
        WHERE id = ?2), 'scheduled')
";

pub(in crate::runtime::store) const CANCEL_SCHEDULED: &str = r"
UPDATE messages
SET state = 'undeliverable'
WHERE id = ?1
    AND state = 'scheduled'
";

pub(super) const MOVE_SCHEDULED: &str = r"
DELETE FROM messages
WHERE seq = ?1
    AND id = ?2
    AND state = 'scheduled'
";

pub(super) const MOVE_SCHEDULED_2: &str = "\nINSERT INTO messages(seq, id, sender, recipient, \
    body, state, startup, mode, redirect, kind, continuation_root,\n        \
    continuation_attempt, not_before, schedule_integrity, join_id)\nVALUES (?1, ?2, ?3, ?4, \
    ?5, ?6, 0, 'next-turn', 0, NULL, NULL, NULL, ?7, ?8, ?9)\n";

pub(super) const SCHEDULED_PROOF: &str = "\nSELECT (d.message_id IS NOT NULL\n        AND \
    d.status = 'scheduled'\n        AND d.error IS NULL\n        AND d.execution IS NULL\n        \
    AND d.provider IS NULL\n        AND d.thread IS NULL\n        AND d.recipient_turn IS \
    NULL\n        AND d.call_expired = 0\n        AND c.start_seq IS NULL\n        AND \
    c.output IS NULL\n        AND coalesce(c.output_final, 0) = 0\n        AND (\n            \
    (?2 = 'human' AND c.message_id IS NOT NULL AND d.sender_turn IS NULL\n                AND \
    typeof(c.accepted_at) = 'integer' AND c.accepted_at >= 0)\n            OR (?2 != 'human' \
    AND c.message_id IS NULL AND NOT EXISTS (\n                SELECT 1 FROM messages WHERE \
    id=?1 AND join_id IS NOT NULL))\n            OR (?2 != 'human' AND c.message_id IS NOT \
    NULL\n                AND typeof(c.accepted_at)='integer' AND c.accepted_at>=0 AND EXISTS \
    (\n                    SELECT 1 FROM join_members jm JOIN messages m\n                    \
    ON m.id=jm.message_id AND m.join_id=jm.join_id\n                    WHERE \
    jm.message_id=?1 AND jm.requester=?2)))), (coalesce(d.status NOT IN ('scheduled', \
    'accepted'),\n            0)\n        OR d.execution IS NOT NULL\n        OR d.provider \
    IS NOT NULL\n        OR d.thread IS NOT NULL\n        OR d.recipient_turn IS NOT NULL\n        \
    OR coalesce(d.call_expired, 0) != 0\n        OR c.start_seq IS NOT NULL\n        OR \
    c.output IS NOT NULL\n        OR coalesce(c.output_final, 0) != 0)\nFROM (\n    SELECT \
    1)\nLEFT JOIN deliveries d\nON d.message_id = ?1\nLEFT JOIN call_results c\nON \
    c.message_id = ?1\n";

pub(super) const PROMOTE_SCHEDULED: &str = r"
UPDATE deliveries
SET status = ?1
WHERE message_id = ?2
";
