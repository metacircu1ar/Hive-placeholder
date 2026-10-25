//! Fixed per-service counters; no rejected request or lost reply is retained.
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy)]
pub(super) enum Loss {
    FrameCapacity,
    PeerGone,
    ReplyTimeout,
    ReplyFailure,
    QueryReply,
    WriteAdmission,
}
impl Loss {
    const ALL: [Self; 6] = [
        Self::FrameCapacity,
        Self::PeerGone,
        Self::ReplyTimeout,
        Self::ReplyFailure,
        Self::QueryReply,
        Self::WriteAdmission,
    ];
    fn name(self) -> &'static str {
        match self {
            Self::FrameCapacity => "frame_capacity",
            Self::PeerGone => "peer_gone",
            Self::ReplyTimeout => "reply_timeout",
            Self::ReplyFailure => "reply_failure",
            Self::QueryReply => "query_reply",
            Self::WriteAdmission => "write_admission",
        }
    }
}

#[derive(Default)]
pub(super) struct Counters([AtomicU64; 6]);
impl Counters {
    pub fn lost(&self, loss: Loss, error: Option<&(dyn std::error::Error + 'static)>) {
        let previous =
            self.0[loss as usize]
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1));
        // Saturation is a final exact count, not a new observable loss transition.
        let Ok(previous) = previous else { return };
        let count = previous + 1;
        if !count.is_power_of_two() {
            return;
        }
        if matches!(loss, Loss::FrameCapacity | Loss::WriteAdmission) {
            tracing::debug!(name: "comb.admission_rejected",
                 phase = loss.name(),
                 count = count,
                 error = error,
                 decision = "retry_with_backoff");
        } else if matches!(loss, Loss::ReplyFailure) {
            tracing::warn!(name: "comb.delivery_lost",
                 phase = loss.name(),
                 count = count,
                 error = error,
                 decision = "do_not_replay");
        } else {
            tracing::debug!(name: "comb.delivery_lost",
                 phase = loss.name(),
                 count = count,
                 error = error,
                 decision = "do_not_replay");
        }
    }
    pub fn finish(&self) {
        for loss in Loss::ALL {
            let count = self.0[loss as usize].load(Ordering::Relaxed);
            if count > 0 {
                if matches!(loss, Loss::FrameCapacity | Loss::WriteAdmission) {
                    tracing::info!(name: "comb.admission_rejection_total",
                         phase = loss.name(),
                         count = count);
                } else {
                    tracing::info!(name: "comb.delivery_loss_total",
                         phase = loss.name(),
                         count = count);
                }
            }
        }
    }
    #[cfg(test)]
    pub fn count(&self, loss: Loss) -> u64 {
        self.0[loss as usize].load(Ordering::Relaxed)
    }
}

pub(super) fn joined(result: Result<(), tokio::task::JoinError>, phase: &'static str) {
    if let Err(error) = result {
        if error.is_cancelled() {
            tracing::debug!(name: "comb.task_cancelled", phase = phase, decision = "release_owner");
        } else {
            let cause = crate::error::Cause::worker(error);
            tracing::error!(name: "comb.task_failed",
                 phase = phase,
                 error = &cause as &(dyn std::error::Error + 'static),
                 decision = "release_owner");
        }
    }
}

pub(super) fn peer_gone(error: &crate::Error) -> bool {
    // Framing writes retain their native I/O error directly; an encoding or other
    // failure must not be downgraded just because a nested cause names a peer.
    use std::error::Error as _;
    error
        .source()
        .and_then(|e| e.downcast_ref::<std::io::Error>())
        .is_some_and(|e| {
            matches!(
                e.kind(),
                std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::NotConnected
            )
        })
}

/// Exhaustive fixed protocol labels; never format a request or its private fields.
pub(super) fn method(operation: &super::Operation) -> &'static str {
    use super::Operation;
    match operation {
        Operation::Capabilities => "capabilities",
        Operation::Health => "health",
        Operation::Get { .. } => "get",
        Operation::Search { .. } => "search",
        Operation::List { .. } => "list",
        Operation::Links { .. } => "links",
        Operation::Graph { .. } => "graph",
        Operation::Stats => "stats",
        Operation::Doctor => "doctor",
        Operation::Put { .. } => "put",
        Operation::Append { .. } => "append",
        Operation::Capture { .. } => "capture",
        Operation::Promote { .. } => "promote",
        Operation::Consolidate { .. } => "consolidate",
        Operation::Decide { .. } => "decide",
        Operation::Remove { .. } => "remove",
        Operation::Sync => "sync",
        Operation::Import { .. } => "import",
        Operation::ImportStatus => "import_status",
        Operation::ImportAbandon { .. } => "import_abandon",
        Operation::CredentialCreate { .. } => "credential_create",
        Operation::CredentialList => "credential_list",
        Operation::CredentialRevoke { .. } => "credential_revoke",
    }
}
