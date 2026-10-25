//! Calls SQL statements and query builders.

pub(super) const TRACK_CALL: &str = r"
INSERT INTO call_results(message_id, accepted_at)
VALUES (?1, ?2)
";

pub(super) const ASSISTANT_OUTPUT: &str = r"
UPDATE call_results
SET output = ?1, output_final = ?2
WHERE message_id = ?3
    AND (output_final = 0
        OR ?2 = 1)
    AND EXISTS (
    SELECT 1
    FROM deliveries d
    JOIN messages m
    ON m.id = d.message_id
        AND (m.sender = 'human' OR m.join_id IS NOT NULL)
    WHERE d.message_id = ?3
        AND d.execution = ?4
        AND d.recipient_turn = ?5
        AND d.status = 'submitted'
        AND m.recipient = ?6)
";

pub(super) const MESSAGE_RESULT: &str = "\nSELECT m.recipient, m.state, m.seq, m.startup, \
    m.redirect, m.mode, d.execution, d.thread, d.recipient_turn,\n    d.error, \
    d.call_expired, c.message_id, c.start_seq, c.output, a.state, a.paused, EXISTS (\n    \
    SELECT 1\n    FROM messages active\n    WHERE active.recipient = m.recipient\n        AND \
    active.mode = 'next-turn'\n        AND active.state IN ('submitting', 'submitted')), \
    c.output_final, m.not_before, m.schedule_integrity,\n    d.message_id\nFROM messages \
    m\nLEFT JOIN deliveries d\nON d.message_id = m.id\nJOIN agents a\nON a.id = \
    m.recipient\nLEFT JOIN call_results c\nON c.message_id = m.id\nWHERE m.sender = 'human'\n    \
    AND m.id = ?1\n";

pub(in crate::runtime::store) const QUEUE_RANK: &str = r"
SELECT count(*)
FROM (
    SELECT 1
    FROM (
        SELECT seq
        FROM (
            SELECT seq
            FROM messages INDEXED BY messages_pending
            WHERE recipient = ?1
                AND mode = 'next-turn'
                AND state = 'accepted'
                AND startup > ?2
            LIMIT 10001)
        UNION ALL
        SELECT seq
        FROM (
            SELECT seq
            FROM messages INDEXED BY messages_pending
            WHERE recipient = ?1
                AND mode = 'next-turn'
                AND state = 'accepted'
                AND startup = ?2
                AND redirect > ?3
            LIMIT 10001)
        UNION ALL
        SELECT seq
        FROM (
            SELECT seq
            FROM messages INDEXED BY messages_pending
            WHERE recipient = ?1
                AND mode = 'next-turn'
                AND state = 'accepted'
                AND startup = ?2
                AND redirect = ?3
                AND seq < ?4
            LIMIT 10001))
    LIMIT 10001)
";
