//! Build UTF-8 handoff sections with each caller's byte budget and row cutoff.
use super::sql::Cursor;
use crate::error::Result;

pub(super) struct Handoff(String);
impl Handoff {
    pub fn new(header: String) -> Self {
        Self(header)
    }
    pub fn section(&mut self, title: &str) {
        self.0.push_str(title);
        self.0.push_str(":\n");
    }
    pub fn remaining(&self, limit: usize) -> usize {
        limit - self.0.len()
    }
    pub fn rows(
        &mut self,
        rows: &mut Cursor<'_>,
        mut remaining: usize,
        minimum: usize,
    ) -> Result<()> {
        // Decode the next row before testing the cutoff, as both callers did:
        // malformed stored context must still fail at that observation boundary.
        while let Some(row) = rows.column::<String>()? {
            if remaining < minimum {
                break;
            }
            append_bounded(&mut self.0, &row, &mut remaining);
        }
        Ok(())
    }
    pub fn finish(self) -> String {
        self.0
    }
}

fn append_bounded(out: &mut String, row: &str, remaining: &mut usize) {
    let mut end = row.len().min(*remaining - 1);
    while !row.is_char_boundary(end) {
        end -= 1;
    }
    out.push_str(&row[..end]);
    out.push('\n');
    *remaining -= end + 1;
}

#[cfg(test)]
// Stored-row fixtures and byte assertions deliberately panic on failure.
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::runtime::store::sql::Database;
    use rusqlite::{Connection, params};

    #[test]
    fn handoff_sections_preserve_utf8_newlines_and_independent_budgets() {
        let connection = Connection::open_in_memory().unwrap();
        let mut handoff = Handoff::new("header\n".into());
        Database::new(&connection)
            .with_rows("SELECT ?1 UNION ALL SELECT ?2", ["ééé", "tail"], |rows| {
                handoff.rows(rows, 6, 2)
            })
            .unwrap();
        handoff.section("Open");
        Database::new(&connection)
            .with_rows("SELECT ?1 UNION ALL SELECT ?2", ["z", "finish"], |rows| {
                handoff.rows(rows, 4, 2)
            })
            .unwrap();
        let expected = "header\néé\nOpen:\nz\nf\n";
        assert_eq!(handoff.remaining(100), 100 - expected.len());
        assert_eq!(handoff.finish(), expected);
    }

    #[test]
    fn handoff_observes_the_next_row_error_before_its_budget_cutoff() {
        let connection = Connection::open_in_memory().unwrap();
        let mut handoff = Handoff::new(String::new());
        let error = Database::new(&connection)
            .with_rows(
                "SELECT ?1 UNION ALL SELECT ?2",
                params!["abcd", 7],
                |rows| handoff.rows(rows, 2, 2),
            )
            .unwrap_err();
        assert_eq!(error.code(), "database_error");
        assert_eq!(handoff.finish(), "a\n");
    }

    #[test]
    fn handoff_preserves_distinct_branch_and_auth_tail_cutoffs() {
        let connection = Connection::open_in_memory().unwrap();
        let prefix = "x".repeat(31);
        for (minimum, tail) in [(2, "t\n"), (32, "")] {
            let mut handoff = Handoff::new(String::new());
            Database::new(&connection)
                .with_rows("SELECT ?1 UNION ALL SELECT ?2", [&prefix, "tail"], |rows| {
                    handoff.rows(rows, 34, minimum)
                })
                .unwrap();
            assert_eq!(handoff.finish(), format!("{prefix}\n{tail}"));
        }
    }
}
