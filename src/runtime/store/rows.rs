//! Read SQLite columns in query order; JSON decoding stays at the call site so
//! its error classification and pagination boundaries remain unchanged.
use super::{Message, api};
use rusqlite::{Result, Row};

// Keep each query's column order, named fields and checked decoders together.
macro_rules! record {
    ($(#[$meta:meta])* $vis:vis struct $name:ident {
        $($field_vis:vis $field:ident: $ty:ty = $index:literal $(=> $decode:path)?),* $(,)?
    }) => {
        $(#[$meta])*
        $vis struct $name { $($field_vis $field: $ty),* }
        impl $crate::runtime::store::sql::FromRow for $name {
            fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
                Ok(Self { $($field: $crate::runtime::store::rows::record!(@read row,
                     $index $(,
                     $decode)?)?),
                    * })
            }
        }
    };
    (@read $row:ident, $index:literal, $decode:path) => { $decode($row, $index) };
    (@read $row:ident, $index:literal) => { $row.get($index) };
}
pub(super) use record;

pub(super) use super::convert::{count, nonnegative, optional_unsigned, unsigned};

super::rows::record! {
    pub(super) struct AgentPageRow {
        pub sequence: i64 = 0 => nonnegative,
        pub record: String = 1,
        pub approval_id: Option<String> = 2,
    }
}

super::rows::record! {
    pub(super) struct EventRow {
        pub sequence: i64 = 0 => nonnegative,
        pub execution: Option<String> = 1,
        pub timestamp: i64 = 2 => nonnegative,
        pub kind: crate::runtime::domain::EventKind = 3,
        pub data: String = 4,
    }
}

pub(super) fn message(row: &Row<'_>) -> Result<Message> {
    Ok(Message {
        has_attachments: row.get(6)?,
        attachments: Vec::new(),
        joined: row.get(5)?,
        id: row.get(0)?,
        sender: row.get(1)?,
        recipient: row.get(2)?,
        body: row.get(3)?,
        state: row.get(4)?,
    })
}

pub(super) fn message_record(row: &Row<'_>) -> Result<api::MessageRecord> {
    Ok(api::MessageRecord {
        has_attachments: row.get(13)?,
        attachments: Vec::new(),
        seq: nonnegative(row, 0)?,
        id: row.get(1)?,
        sender: row.get(2)?,
        recipient: row.get(3)?,
        body: row.get(4)?,
        state: row.get(5)?,
        mode: row.get(6)?,
        steering_target: if row.get::<_, crate::runtime::domain::DeliveryMode>(6)?
            == crate::runtime::domain::DeliveryMode::Steer
        {
            Some(api::SteeringTarget {
                execution_id: row.get(7)?,
                turn_id: row.get(8)?,
            })
        } else {
            None
        },
        delivery_error: row.get(9)?,
        kind: row.get(10)?,
        not_before: optional_unsigned(row, 11)?,
        schedule_integrity: row.get(12)?,
    })
}

super::rows::record! {
    pub(super) struct RequestReplay {
        pub body: String = 0,
        pub response: String = 1,
        pub protocol: u32 = 2,
    }
}

super::rows::record! {
    pub(super) struct PrunableEvent {
        pub sequence: i64 = 0 => nonnegative,
        pub agent: String = 1,
    }
}

super::rows::record! {
    pub(super) struct Artifact {
        pub id: String = 0,
        pub bytes: i64 = 1 => nonnegative,
        pub created_at: i64 = 2 => nonnegative,
    }
}

super::rows::record! {
    pub(super) struct AgentRecord {
        pub id: String = 0,
        pub data: String = 1,
    }
}
super::rows::record! {
    pub(super) struct LoginGeneration {
        pub profile: String = 0,
        pub generation: String = 1,
    }
}
super::rows::record! {
    pub(super) struct PurgeAgent {
        pub id: String = 0,
        pub data: String = 1,
        pub claimed: bool = 2,
        pub blocked: bool = 3,
    }
}
super::rows::record! {
    pub(super) struct ArtifactDeletion {
        pub id: String = 0,
        pub indexed: bool = 1,
    }
}
super::rows::record! {
    pub(super) struct ReleasedClaim {
        pub hold: i64 = 0 => nonnegative,
        pub phase: Option<String> = 1,
    }
}
