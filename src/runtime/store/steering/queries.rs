//! Steering SQL statements and query builders.

pub(in crate::runtime::store) const EXPIRE: &str = r"
UPDATE messages
SET state = 'stale_turn'
WHERE recipient = ?1
    AND mode = 'steer'
    AND state = 'accepted'
";

pub(in crate::runtime::store) const FINISH: &str = r"
UPDATE messages
SET state = ?4
WHERE recipient = ?1
    AND mode = 'steer'
    AND state = 'submitted'
    AND EXISTS (
    SELECT 1
    FROM deliveries
    WHERE message_id = messages.id
        AND execution = ?2
        AND recipient_turn = ?3)
";

pub(super) const INSERT_MESSAGE: &str = r"
INSERT INTO messages(id, sender, recipient, body, state, mode)
VALUES (?1, ?2, ?3, ?4, 'accepted', 'steer')
";

pub(super) const INSERT_DELIVERY: &str = "\nINSERT INTO deliveries(message_id, sender_turn, \
    recipient_turn, execution, provider, thread)\nVALUES (?1, (\n        SELECT turn_id\n        \
    FROM agents\n        WHERE id = ?2), ?3, ?4, ?5, ?6)\n";

pub(super) const NEXT_STEERING: &str = "\nSELECT m.id, m.sender, m.recipient, m.body, m.state, \
    m.join_id IS NOT NULL, m.has_attachments\nFROM messages m\nJOIN deliveries d\nON \
    d.message_id = m.id\nWHERE m.recipient = ?1\n    AND m.mode = 'steer'\n    AND m.state = \
    'accepted'\n    AND d.execution = ?2\n    AND d.recipient_turn = ?3\nORDER BY \
    m.seq\nLIMIT 1\n";

pub(super) const SET_MESSAGE_STATE: &str = r"
UPDATE messages
SET state = ?1
WHERE id = ?2
    AND sender = ?3
    AND mode = 'steer'
    AND state = ?4
";

pub(super) const SET_DELIVERY_ERROR: &str = r"
UPDATE deliveries
SET error = ?1
WHERE message_id = ?2
";
