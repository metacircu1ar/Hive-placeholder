//! SQL owned by this store operation; statement text is a profiler key.

pub(super) const CONFIGURE_COLLECTORS_UPDATE: &str = "UPDATE comb_collectors SET enabled = 0";

pub(super) const CONFIGURE_COLLECTORS_INSERT: &str = "INSERT INTO comb_collectors(name, \
credential_file, sources, enabled, comb_id, paused, last_error) VALUES(?1, ?2, ?3, ?4, ?5, ?5 \
IS NULL, CASE WHEN ?5 IS NULL THEN 'identity_unbound' ELSE NULL END)\n                    ON \
CONFLICT(name) DO UPDATE SET credential_file = excluded.credential_file, sources = \
excluded.sources, enabled = excluded.enabled,\n                    revision = \
comb_collectors.revision + 1, retry_at = 0";

pub(super) const CONFIGURE_COLLECTORS_SELECT: &str = "SELECT count(*) FROM comb_collectors";

pub(super) const COLLECTOR_DESTINATIONS_SELECT: &str = "SELECT name, credential_file, revision \
FROM comb_collectors\n            WHERE (pending > 0 OR gap_count > 0) AND paused = 0 AND \
retry_at <= ?1 ORDER BY name";

pub(super) const COLLECTOR_STATUS_SELECT: &str = "SELECT name, enabled, paused, comb_id, \
    credential_id, pending, bytes,\n            (SELECT count(*) FROM comb_outbox o WHERE \
    o.destination = c.name AND o.comb_id IS NOT NULL),\n            gap_count, gap_first, \
    gap_last, last_error FROM comb_collectors c ORDER BY name";

pub(super) const COLLECTOR_STATUS_SELECT_2: &str = "SELECT id, generation, comb_id, \
    credential_id, last_error FROM comb_outbox WHERE destination = ?1 AND comb_id IS NOT NULL \
    ORDER BY sequence LIMIT 1024";
