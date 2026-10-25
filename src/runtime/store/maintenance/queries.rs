//! Shared selection predicates for read-only previews and actual retention.
use std::sync::LazyLock;

pub(super) const EVENT_FRONTIER: &str = r"
SELECT id, min((
        SELECT seq
        FROM events
        WHERE agent = agents.id
        ORDER BY seq DESC
        LIMIT 1
        OFFSET ?1), coalesce((
            SELECT min(seq) - 1
            FROM events
            WHERE agent = agents.id
                AND at >= ?2), 9223372036854775807))
FROM agents
WHERE state = 'stopped'
    AND (task_outcome IN ('completed', 'cancelled')
        OR expired = 1)
";

pub(super) static FILL_COMPACTION_FRONTIER: LazyLock<String> =
    LazyLock::new(|| format!("INSERT INTO compact_frontier {EVENT_FRONTIER}"));
pub(super) static PREVIEW_EVENTS: LazyLock<String> = LazyLock::new(|| {
    format!(
        "WITH frontier(agent,through) AS ({EVENT_FRONTIER})
        SELECT count(*) FROM events e JOIN frontier f ON f.agent=e.agent
        WHERE e.seq > 0 AND e.seq <= f.through AND e.at < ?2"
    )
});

const EXPIRABLE_CALLS: &str = r"
SELECT c.message_id
FROM call_results c JOIN deliveries d ON d.message_id=c.message_id
WHERE c.accepted_at < ?1
    AND d.status IN ('completed', 'interrupted', 'failed', 'uncertain', 'undeliverable')
";
pub(super) static PRUNE_CALLS: LazyLock<String> = LazyLock::new(|| {
    format!("UPDATE deliveries SET call_expired=1 WHERE message_id IN ({EXPIRABLE_CALLS})")
});
pub(super) static PREVIEW_CALLS: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT count(*) FROM call_results c WHERE c.message_id IN ({EXPIRABLE_CALLS})\n        \
            OR EXISTS(SELECT 1 FROM deliveries d WHERE d.message_id=c.message_id AND \
            d.call_expired=1)"
    )
});
pub(super) const PRUNE_CALLS_2: &str = r"
DELETE FROM call_results
WHERE EXISTS (
    SELECT 1 FROM deliveries d
    WHERE d.message_id=call_results.message_id AND d.call_expired=1)
";
pub(super) const PENDING_ARTIFACTS: &str = "SELECT count(*) FROM artifact_deletions";
pub(super) const OUTPUT_OWNERSHIP: &str = r"
SELECT EXISTS(SELECT 1 FROM artifacts WHERE id=?1),
       EXISTS(SELECT 1 FROM artifact_deletions WHERE id=?1)
";
