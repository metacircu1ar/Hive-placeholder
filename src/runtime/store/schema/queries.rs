//! Schema SQL statements and query builders.

pub(super) const EMPTY: &str = r"
SELECT NOT EXISTS (
    SELECT 1
    FROM sqlite_master
    WHERE name NOT GLOB 'sqlite_*')
";

pub(super) const CONFIGURE_DATABASE: &str = r"
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA foreign_keys = ON;
";

pub(super) const BEGIN_INITIALIZE: &str = r"
BEGIN IMMEDIATE
";

pub(super) const COMMIT_INITIALIZE: &str = r"
COMMIT
";

pub(super) const ROLLBACK_INITIALIZE: &str = r"
ROLLBACK
";

pub(super) const OBJECTS: &str = r"
SELECT name, sql
FROM sqlite_master
WHERE name NOT GLOB 'sqlite_*'
ORDER BY name
";

// Both initialization and validation execute this single current definition.
pub(super) static SCHEMA: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| assemble(include_str!("../schema.sql")));

fn pending(prefix: &str) -> String {
    format!("{prefix}state = 'accepted' AND {prefix}mode = 'next-turn'")
}

fn mail(prefix: &str) -> String {
    format!("{prefix}state = 'accepted' AND {prefix}startup = 0 AND {prefix}mode = 'next-turn'")
}

fn pending_sequence(owner: &str) -> String {
    let pending = pending("");
    format!(
        r"
SELECT min(seq)
FROM messages
WHERE recipient = {owner} AND {pending}
"
    )
}

fn head_startup(owner: &str) -> String {
    let pending = pending("");
    format!(
        r"
SELECT startup
FROM messages
WHERE recipient = {owner} AND {pending}
ORDER BY startup DESC, redirect DESC, seq
LIMIT 1
"
    )
}

fn active(row: &str) -> String {
    format!(
        "NOT({row}.task_outcome IN ('completed', 'cancelled') OR {row}.expired = 1) \
            OR {row}.state != 'stopped'"
    )
}

pub(super) fn assemble(template: &str) -> String {
    let mut schema = template.to_owned();
    for row in ["new", "old"] {
        let prefix = format!("{row}.");
        let recipient = format!("{row}.recipient");
        for (name, expression) in [
            ("pending", pending(&prefix)),
            ("mail", mail(&prefix)),
            ("pending_sequence", pending_sequence(&recipient)),
            ("head_startup", head_startup(&recipient)),
            ("active", active(row)),
        ] {
            schema = schema.replace(&format!("{{{{{name}:{row}}}}}"), &expression);
        }
    }
    assert!(
        !schema.contains("{{") && !schema.contains("}}"),
        "Unknown or unexpanded current schema expression"
    );
    schema
}

pub(in crate::runtime::store) fn refresh_mail(owner: &str) -> String {
    let pending = pending("");
    let mail = mail("");
    let sequence = pending_sequence(owner);
    let head = head_startup(owner);
    format!(
        r"
UPDATE agents
SET pending_count = (
    SELECT count(*)
    FROM messages
    WHERE recipient = {owner} AND {pending}),
    mail_count = (
    SELECT count(*)
    FROM messages
    WHERE recipient = {owner} AND {mail}),
    pending_seq = ({sequence}),
    head_startup = ({head})
WHERE id = {owner};
"
    )
}
