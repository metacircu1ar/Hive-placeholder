//! SQL owned by this store operation; statement text is a profiler key.

pub(super) const COLLECT_EVIDENCE_SELECT: &str =
    "SELECT name, enabled, sources FROM comb_collectors ORDER BY name";

pub(super) const COLLECT_EVIDENCE_SELECT_2: &str =
    "SELECT identity FROM runtime_identity WHERE id = 1";

pub(super) const COLLECT_EVIDENCE_SELECT_3: &str =
    "SELECT gap_count FROM comb_collectors WHERE name = ?1";

pub(super) const COLLECT_EVIDENCE_UPDATE: &str = "UPDATE comb_collectors SET gap_count = \
    gap_count + 1, gap_first = coalesce(gap_first, ?2), gap_last = ?2 WHERE name = ?1";

pub(super) const COLLECTOR_CAPACITY_SELECT: &str = "SELECT pending < ?2 AND bytes + ?3 <= ?4 \
    AND gap_count = 0 FROM comb_collectors WHERE name = ?1";

pub(super) const INSERT_COLLECTION_INSERT: &str = "INSERT INTO comb_outbox(id, destination, \
    sequence, body, bytes) VALUES(?1, ?2, ?3, ?4, ?5)";

pub(super) const COLLECTOR_GAP_SELECT: &str = "SELECT gap_count, gap_first, gap_last, pending, \
    bytes FROM comb_collectors WHERE name = ?1";

pub(super) const COLLECTOR_GAP_SELECT_2: &str =
    "SELECT identity FROM runtime_identity WHERE id = 1";

pub(super) const COLLECTOR_GAP_UPDATE: &str = "UPDATE comb_collectors SET gap_count = 0, \
    gap_first = NULL, gap_last = NULL WHERE name = ?1";
