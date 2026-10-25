//! Purge scope SQL definitions.

pub(super) const FILL_RECURSIVE_SCOPE: &str = r"
WITH RECURSIVE tree(id) AS (
    VALUES (?1)
    UNION SELECT a.id
    FROM agents a
    JOIN tree t
    ON a.parent_id = t.id
    UNION SELECT p.id
    FROM purged_agents p
    JOIN tree t
    ON p.parent_id = t.id)
INSERT INTO purge_scope
SELECT id
FROM tree
";

pub(super) const SCOPE_HAS_CYCLE: &str = r"
SELECT EXISTS (
    SELECT 1
    FROM agents
    WHERE id = ?1
        AND parent_id IN purge_scope
    UNION ALL
    SELECT 1
    FROM purged_agents
    WHERE id = ?1
        AND parent_id IN purge_scope)
";

pub(super) const SCOPE_HAS_INVALID_ID: &str = r"
SELECT EXISTS (
    SELECT 1
    FROM purge_scope
    WHERE id IS NULL
        OR id = '')
";

pub(super) const REMOVE_SCOPE_TOMBSTONES: &str = r"
DELETE FROM purge_scope
WHERE id NOT IN (
    SELECT id
    FROM agents)
";

pub(super) const CREATE_PURGE_SCOPE: &str = r"
CREATE TEMP TABLE purge_scope(id TEXT PRIMARY KEY);
CREATE TEMP TABLE purge_snapshots(id INTEGER PRIMARY KEY);
";

pub(super) const PREPARE_PURGE_SNAPSHOTS: &str = r"
INSERT INTO purge_snapshots
SELECT DISTINCT snapshot
FROM agent_instructions
WHERE agent IN purge_scope
";

pub(super) const CLEAR_PURGE_SCOPE: &str = r"
DROP TABLE purge_scope;
DROP TABLE purge_snapshots;
";

pub(super) const PURGE_COUNTS_SELECT: &str = "SELECT coalesce(sum(b.bytes),0) FROM \
    attachment_blobs b WHERE EXISTS (SELECT 1 FROM message_attachments a JOIN messages m ON \
    m.id=a.message_id WHERE a.snapshot_key=b.key AND m.recipient IN purge_scope) AND NOT \
    EXISTS (SELECT 1 FROM message_attachments a JOIN messages m ON m.id=a.message_id WHERE \
    a.snapshot_key=b.key AND m.recipient NOT IN purge_scope)";
