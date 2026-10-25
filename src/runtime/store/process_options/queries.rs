//! SQL owned by this store operation; statement text is a profiler key.

pub(super) const ADMITS_PROCESS_KEYS_SELECT: &str =
    "SELECT v.process_options FROM process_variants v
             LEFT JOIN agents a ON a.id=?2 AND a.auth=v.auth AND a.ended_at IS NULL
             WHERE v.auth=?1 AND v.refs >
                 (CASE WHEN a.process_options=v.process_options THEN 1 ELSE 0 END
                  + CASE WHEN a.reserved_process_options=v.process_options THEN 1 ELSE 0 END)
             LIMIT ?3";
