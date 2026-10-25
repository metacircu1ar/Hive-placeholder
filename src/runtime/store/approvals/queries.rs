//! Approvals SQL statements and query builders.

pub(super) const PENDING_APPROVAL: &str = r"
SELECT id
FROM approvals
WHERE agent = ?1
    AND state = 'pending'
";

pub(super) const APPROVAL: &str = "\nSELECT id, agent, execution, turn, title, message, \
    schema, destructive, state, action, content,\n    reason, created_at, closed_at, \
    response_message\nFROM approvals\nWHERE id = ?1\n";

pub(super) fn approvals(scoped: bool, all: bool) -> String {
    let scope = if scoped { "a.agent=?1 AND " } else { "" };
    let state = if all { "" } else { "a.state='pending' AND " };
    format!(
        "\nSELECT a.id, a.agent, g.role, a.title, a.destructive, a.state, a.action, \
            a.created_at, a.closed_at,\n    g.name\nFROM approvals a\nJOIN(\n    \
            SELECT id, json_extract(data, '$.role') AS role, json_extract(data, \
            '$.name') AS name\n    FROM agents) g\nON g.id = a.agent\nWHERE {scope} \
            {state} (a.created_at, a.id) > (?2, ?3)\nORDER BY a.created_at, \
            a.id\nLIMIT ?4\n"
    )
}

pub(super) const INSERT_APPROVAL: &str = "\nINSERT INTO approvals(id, agent, execution, turn, \
    title, message, schema, destructive, state,\n        created_at)\nVALUES (?1, ?2, ?3, ?4, \
    ?5, ?6, ?7, ?8, 'pending', ?9)\n";

pub(super) const CLOSE_PENDING_APPROVAL: &str = r"
UPDATE approvals
SET state = ?2, closed_at = ?3
WHERE id = ?1
    AND state = 'pending'
";

pub(super) const ANSWER_APPROVAL: &str = "\nUPDATE approvals\nSET state = 'answered', action = \
    ?2, content = ?3, reason = ?4, closed_at = ?5, response_message = ?6\nWHERE id = ?1\n    \
    AND state = 'pending'\n";

pub(super) const ENQUEUE_APPROVAL_ANSWER: &str = r"
UPDATE messages
SET kind = 'approval_answer'
WHERE sender = 'human'
    AND id = ?1
";
