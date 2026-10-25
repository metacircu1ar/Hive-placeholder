//! Persist execution receipts and inspect ownership before recovery or credential reuse.
mod lease;
mod model;
mod ownership;
mod receipt;

pub(super) use lease::lease;
pub use lease::{ProviderEvidence, RuntimeLease};
pub(super) use model::{Class, ExecutionPhase, Record};
pub(in crate::state) use ownership::{check_writers, migration_records};
pub(super) use ownership::{
    check_writers_for_operation, conversation_proven, conversation_safe, find_record,
    migration_safe, ownership_gone, reconcile, records, records_in, records_owned, reservations,
};
pub(super) use receipt::Receipt;

use super::{
    Binding, Error, Lock, LockIdentity, Path, ProcessIdentity, RecoveryReservation, Result, State,
    Uuid, credential, exists, layout, recovery_required,
};
use crate::{
    observability::state_sync::{self as costs, LedgerPhase},
    state::{boot_identity, check_path, corrupt, directory, read_json, same_boot, write_json},
};

#[cfg(test)]
mod tests;
