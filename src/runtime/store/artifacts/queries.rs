//! Artifacts SQL definitions.

pub(super) const PENDING_ARTIFACT_DELETIONS: &str = r"
SELECT d.id, EXISTS (
    SELECT 1
    FROM artifacts a
    WHERE a.id = d.id)
FROM artifact_deletions d
ORDER BY d.id
LIMIT 128
";

pub(super) const ACKNOWLEDGE_ARTIFACT_DELETIONS: &str = r"
DELETE FROM artifact_deletions
WHERE id = ?1
";
