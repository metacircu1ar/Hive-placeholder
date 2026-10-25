//! Agent-owned table inventory shared by purge and GC SQL generation.
// Deletion order keeps mailbox proofs and snapshot bindings ahead of their owners.
// GC charges only the historical work set.
pub(super) type CountField = fn(&mut super::api::PurgeCounts) -> &mut usize;
pub(super) struct OwnedTable {
    pub table: &'static str,
    pub owner: Ownership,
    pub count: Option<CountField>,
    pub gc: bool,
}
pub(super) enum Ownership {
    Column(&'static str),
    Incoming,
    Calls,
    Snapshots,
}
macro_rules! owned_tables {
    ($(($table:literal, $owner:expr, $count:expr, $gc:expr)),* $(,)?) => {
        pub(super) const OWNED_TABLES: &[OwnedTable] = &[$(OwnedTable {
            table: $table,
            owner: $owner,
            count: $count,
            gc: $gc,
        }),*];
    };
}
owned_tables! {
    ("agent_image_usage", Ownership::Column("agent"), None, true),
    ("branches", Ownership::Column("agent"), None, false),
    ("artifacts", Ownership::Column("agent"), Some(|c| &mut c.artifacts), true),
    ("call_results", Ownership::Calls, Some(|c| &mut c.call_results), true),
    ("join_members", Ownership::Column("requester"), Some(|c| &mut c.join_members), true),
    ("joins", Ownership::Column("requester"), Some(|c| &mut c.joins), true),
    ("approvals", Ownership::Column("agent"), Some(|c| &mut c.approvals), true),
    ("deliveries", Ownership::Incoming, Some(|c| &mut c.deliveries), true),
    ("message_attachments", Ownership::Incoming, Some(|c| &mut c.attachments), true),
    ("messages", Ownership::Column("recipient"), Some(|c| &mut c.messages), true),
    ("events", Ownership::Column("agent"), Some(|c| &mut c.events), true),
    ("event_retention", Ownership::Column("agent"), Some(|c| &mut c.event_retention), true),
    ("redirect_interrupts", Ownership::Column("agent"), Some(|c| &mut c.redirects), true),
    ("requests", Ownership::Column("sender"), Some(|c| &mut c.requests), true),
    (
        "agent_instructions",
        Ownership::Column("agent"),
        Some(|c| &mut c.instruction_bindings),
        true
    ),
    (
        "instruction_snapshots",
        Ownership::Snapshots,
        Some(|c| &mut c.instruction_snapshots),
        false
    ),
    ("agents", Ownership::Column("id"), Some(|c| &mut c.agents), false),
}
