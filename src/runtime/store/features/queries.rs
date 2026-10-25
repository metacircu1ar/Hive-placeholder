//! SQL owned by this store operation; statement text is a profiler key.

// A pending join does not stop an agent from executing, but a native branch
// source hold does. Reuse the execution predicate for calls and join members.
macro_rules! execution_hold {
    ($agent:literal) => {
        concat!(
            $agent,
            ".task_outcome='open' AND ",
            $agent,
            ".expired=0 AND (",
            $agent,
            ".comb_bound=1 OR EXISTS(SELECT 1 FROM branches b JOIN agents child ",
            "ON child.id=b.agent WHERE b.source=",
            $agent,
            ".id AND b.holding=1 AND child.comb_bound=1 AND child.task_outcome='open' \
                AND child.expired=0))"
        )
    };
}
pub(super) const RECIPIENT_HOLD: &str = concat!(
    "SELECT EXISTS(SELECT 1 FROM agents a WHERE a.id=?1 AND ",
    execution_hold!("a"),
    ")"
);
pub(super) const AGENT_HOLD: &str = concat!(
    "SELECT EXISTS(SELECT 1 FROM agents a WHERE a.id=?1 AND \
        a.task_outcome='open' AND a.expired=0 AND (",
    execution_hold!("a"),
    " OR EXISTS(SELECT 1 FROM joins j JOIN join_members m ON m.join_id=j.id \
        JOIN agents t ON t.id=m.target WHERE j.requester=a.id AND j.state IN \
        ('collecting','deadline_passed') AND m.outcome='pending' AND ",
    execution_hold!("t"),
    ")))"
);

pub(super) const RETAINED_COMB_BACKLOG_SELECT: &str =
    "SELECT (SELECT count(*) FROM comb_collectors), (SELECT count(*) FROM comb_outbox)";

#[cfg(not(feature = "comb"))]
pub(super) const DECORATE_HOLDS_SELECT: &str =
    "SELECT recipient FROM messages WHERE id=?1 AND state IN ('accepted','scheduled')";
