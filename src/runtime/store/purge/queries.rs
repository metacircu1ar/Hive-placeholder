//! Purge SQL statements and query builders.

pub(super) const WAS_PURGED: &str = r"
SELECT EXISTS (
    SELECT 1
    FROM purged_agents
    WHERE id = ?1)
";

pub(super) const PURGE: &str = r"
SELECT a.id, v.data, a.claimed, a.blocked
FROM agents a
JOIN purge_scope p
ON p.id = a.id
JOIN agent_records v
ON v.id = a.id
WHERE a.id > ?1
ORDER BY a.id
LIMIT 128
";

pub(super) const HAS_PENDING_INPUT: &str = r"
SELECT EXISTS (
    SELECT 1
    FROM messages
    WHERE recipient = ?1
        AND state IN ('accepted', 'submitting', 'submitted'))
";

pub(super) const RESERVED_INPUT_TERMINAL: &str = "\nSELECT state IN ('completed', \
    'interrupted', 'failed', 'uncertain', 'undeliverable', 'stale_turn')\nFROM \
    messages\nWHERE recipient = ?1\n    AND id = ?2\n";
