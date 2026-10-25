//! SQL owned by this store operation; statement text is a profiler key.

pub(super) const CLAIM_COLLECTION_SELECT: &str =
    "SELECT comb_id, revision, paused FROM comb_collectors WHERE name = ?1";

pub(super) const CLAIM_COLLECTION_UPDATE: &str = "UPDATE comb_collectors SET paused = 1, \
    last_error = 'comb_identity_changed' WHERE name = ?1";

pub(super) const CLAIM_COLLECTION_UPDATE_2: &str = "UPDATE comb_collectors SET comb_id = ?2, \
    credential_id = ?3, last_error = NULL, retry_at = 0 WHERE name = ?1";

pub(super) const CLAIM_COLLECTION_SELECT_2: &str = "SELECT id, generation, body FROM \
    comb_outbox WHERE destination = ?1 AND retry_at <= ?2 AND (comb_id IS NULL OR (comb_id = \
    ?3 AND credential_id = ?4)) ORDER BY sequence, id LIMIT 1";

pub(super) const CLAIM_COLLECTION_UPDATE_3: &str =
    "UPDATE comb_outbox SET comb_id = ?2, credential_id = ?3 WHERE id = ?1";

pub(super) const CLAIM_COLLECTION_UPDATE_4: &str = "UPDATE comb_collectors SET retry_at = ?2, \
    last_error = CASE WHEN EXISTS(SELECT 1 FROM comb_outbox WHERE destination = ?1 AND \
    comb_id IS NOT NULL AND (comb_id <> ?3 OR credential_id <> ?4)) THEN \
    'uncertain_principal_changed' ELSE NULL END WHERE name = ?1";

pub(super) const COLLECTION_DESTINATION_ERROR_UPDATE: &str = "UPDATE comb_collectors SET \
    last_error = ?3, retry_at = ?4 WHERE name = ?1 AND revision = ?2";

pub(super) const FINISH_COLLECTION_UPDATE: &str = "UPDATE comb_outbox SET last_error = ?3, \
    retry_at = ?4 WHERE id = ?1 AND generation = ?2 AND EXISTS(SELECT 1 FROM comb_collectors \
    WHERE name = ?5 AND revision = ?6)";

pub(super) const FINISH_COLLECTION_DELETE: &str = "DELETE FROM comb_outbox WHERE id = ?1 AND \
    generation = ?2 AND EXISTS(SELECT 1 FROM comb_collectors WHERE name = ?3 AND revision = \
    ?4)";

pub(super) const FINISH_COLLECTION_UPDATE_2: &str =
    "UPDATE comb_collectors SET last_error = ?2 WHERE name = ?1";

pub(super) const COLLECTOR_ADMIN_SELECT: &str =
    "SELECT EXISTS(SELECT 1 FROM comb_collectors WHERE name = ?1)";

pub(super) const COLLECTOR_ADMIN_DELETE: &str = "DELETE FROM comb_outbox WHERE destination = ?1";

pub(super) const COLLECTOR_ADMIN_UPDATE: &str = "UPDATE comb_collectors SET gap_count = 0, \
    gap_first = NULL, gap_last = NULL, revision = revision + 1, retry_at = 0, last_error = \
    NULL WHERE name = ?1";

pub(super) const COLLECTOR_ADMIN_UPDATE_2: &str = "UPDATE comb_outbox SET comb_id = NULL, \
    credential_id = NULL, generation = generation + 1, retry_at = 0, last_error = NULL WHERE \
    id = ?1 AND destination = ?2 AND comb_id IS NOT NULL";

pub(super) const COLLECTOR_ADMIN_UPDATE_3: &str =
    "UPDATE comb_collectors SET retry_at = 0 WHERE name = ?1";

pub(super) const COLLECTOR_ADMIN_UPDATE_4: &str = "UPDATE comb_collectors SET comb_id = ?2, \
    credential_id = NULL, paused = 0, revision = revision + 1, retry_at = 0, last_error = \
    NULL WHERE name = ?1";
