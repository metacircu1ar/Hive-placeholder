//! Redirect SQL statements and query builders.

pub(super) const ENQUEUE_REDIRECT: &str = r"
INSERT INTO redirect_interrupts(agent, execution, input_message, turn, state)
VALUES (?1, ?2, ?3, ?4, ?5)
ON CONFLICT (execution, input_message) DO NOTHING
";

pub(super) const REDIRECT_TARGET: &str = r"
SELECT m.id
FROM messages m
JOIN deliveries d
ON d.message_id = m.id
WHERE m.recipient = ?1
    AND m.mode = 'next-turn'
    AND ((m.state = 'submitted'
            AND d.execution = ?2)
        OR (m.state = 'submitting'
            AND m.id = ?3))
ORDER BY m.seq DESC
LIMIT 1
";

pub(super) const CLAIM_REDIRECT: &str = r"
UPDATE redirect_interrupts
SET state = ?5, turn = ?4
WHERE agent = ?1
    AND execution = ?2
    AND input_message = ?3
    AND state = 'pending'
    AND (turn IS NULL
        OR turn = ?4)
";

pub(super) const FINISH_REDIRECT: &str = r"
UPDATE redirect_interrupts
SET state = ?3
WHERE execution = ?1
    AND input_message = ?2
    AND state IN ('pending', 'sent')
";

pub(super) const LOSE_REDIRECTS: &str = r"
UPDATE redirect_interrupts
SET state = ?3
WHERE agent = ?1
    AND execution = ?2
    AND state IN ('pending', 'sent')
";
