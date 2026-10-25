//! Gc SQL statements and query builders.

pub(super) const CANDIDATE_PAGE: &str = r"
SELECT id, ended_at
FROM agents
WHERE ended_at IS NOT NULL
    AND ended_at < ?1
    AND (ended_at, id) > (?2, ?3)
ORDER BY ended_at, id
LIMIT 128
";

pub(super) const HAS_MORE: &str = r"
SELECT EXISTS (
    SELECT 1
    FROM agents
    WHERE ended_at IS NOT NULL
        AND ended_at < ?1
        AND (ended_at, id) > (?2, ?3))
";

pub(super) const SCOPE_ARTIFACT_BYTES: &str = r"
SELECT coalesce(sum(bytes), 0)
FROM artifacts
WHERE agent IN purge_scope
";

pub(super) const GC_BLOCKER: &str = r"
SELECT claimed, blocked
FROM agents
WHERE id = ?1
";
