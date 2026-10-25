//! SQL owned by this store operation; statement text is a profiler key.

pub(super) const AUTH_LOAD_SELECT: &str =
    "SELECT coalesce((SELECT bound_agents FROM auth_load WHERE profile=?1),0),
                (SELECT count(*) FROM agents WHERE auth=?1 AND turn_id IS NOT NULL)";
