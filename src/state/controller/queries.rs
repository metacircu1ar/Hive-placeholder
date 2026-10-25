//! Controller SQL statements and query builders.

pub(super) const EXPORT_RETIRED: &str = r"
PRAGMA integrity_check
";

pub(super) fn table_rows(table: &str) -> String {
    format!(
        r"
SELECT *
FROM {table}
"
    )
}
