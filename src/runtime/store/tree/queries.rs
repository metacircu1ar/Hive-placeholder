//! SQL owned by this store operation; statement text is a profiler key.

// A subtree from one root, breadth-first in listing order. The recursive part's
// ORDER BY makes its queue breadth-first and its LIMIT stops the walk, so a
// large subtree costs only the rows returned.
pub(super) const SUBTREE: &str = r"
WITH RECURSIVE tree(id, depth, listing) AS (
    SELECT id, 0, listing_seq
    FROM agents
    WHERE id = ?1
    UNION ALL
    SELECT a.id, t.depth + 1, a.listing_seq
    FROM agents a
    JOIN tree t ON a.parent_id = t.id
    ORDER BY 2, 3
    LIMIT ?2
)
SELECT id
FROM tree
ORDER BY depth, listing
";

// Every live agent, in listing order: parents before their children.
pub(super) const LIVE_AGENTS: &str = r"
SELECT id
FROM agents
WHERE ended_at IS NULL
ORDER BY listing_seq
LIMIT ?1
";

pub(super) const LIVE_ROOTS: &str = r"
SELECT id
FROM agents
WHERE parent_id IS NULL
    AND ended_at IS NULL
ORDER BY listing_seq
LIMIT ?1
";

pub(super) const COUNTS: &str = r"
SELECT pending_count, mail_count, (
    SELECT count(*)
    FROM agents c
    WHERE c.parent_id = a.id)
FROM agents a
WHERE a.id = ?1
";

// The sequence before an agent's newest `limit` events, or 0 for fewer.
pub(super) const TAIL_AFTER: &str = r"
SELECT seq
FROM events
WHERE agent = ?1
ORDER BY seq DESC
LIMIT 1 OFFSET ?2
";
