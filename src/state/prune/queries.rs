//! Prune SQL statements and query builders.

pub(super) const NO_AGENT_REFERENCES: &str = r"
SELECT json_extract(data, '$.auth_generation')
FROM agents
";
