//! Bounded metadata-only notices become visible only after the outer commit.
use super::{Agent, EventKind, Store};
use crate::runtime::domain::ProviderEventKind;

const PENDING_LIMIT: usize = 512;

#[derive(Default)]
pub(super) struct Notices {
    pending: Vec<Notice>,
    dropped: u64,
    marks: Vec<(usize, u64)>,
}
enum Notice {
    Maintenance {
        gc: bool,
        count: u64,
    },
    Agent {
        id: String,
        execution: Option<String>,
        kind: EventKind,
    },
    Provider {
        id: String,
        kind: ProviderEventKind,
    },
}
impl Notices {
    pub(super) fn begin(&mut self) {
        self.marks.push((self.pending.len(), self.dropped));
    }
    pub(super) fn commit(&mut self) {
        self.marks.pop();
        if self.marks.is_empty() {
            for notice in self.pending.drain(..) {
                notice.emit();
            }
            if self.dropped != 0 {
                tracing::warn!(name: "store.lifecycle_notices_dropped",
                     dropped = self.dropped,
                     decision = "inspect_durable_events");
                self.dropped = 0;
            }
        }
    }
    pub(super) fn rollback(&mut self) {
        if let Some((length, dropped)) = self.marks.pop() {
            self.pending.truncate(length);
            self.dropped = dropped;
        }
    }
    pub(super) fn discard(&mut self) {
        self.pending.clear();
        self.marks.clear();
        self.dropped = 0;
    }
    fn record(&mut self, notice: impl FnOnce() -> Notice) {
        if self.marks.is_empty() {
            notice().emit(); // A successful standalone statement has already committed.
        } else if self.pending.len() < PENDING_LIMIT {
            self.pending.push(notice());
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }
}
impl Store {
    pub(super) fn maintenance_notice(&self, gc: bool, count: u64) {
        if enabled(true) {
            self.notices
                .borrow_mut()
                .record(|| Notice::Maintenance { gc, count });
        }
    }
    pub(super) fn agent_notice(&self, agent: &Agent, kind: EventKind) {
        if !enabled(informative(kind)) {
            return;
        }
        self.notices.borrow_mut().record(|| Notice::Agent {
            id: agent.id.clone(),
            execution: agent.execution_id.clone(),
            kind,
        });
    }
    pub(super) fn provider_notice(&self, id: &str, kind: ProviderEventKind) {
        if !enabled(provider_informative(kind)) {
            return;
        }
        self.notices.borrow_mut().record(|| Notice::Provider {
            id: id.into(),
            kind,
        });
    }
}
fn enabled(info: bool) -> bool {
    if info {
        tracing::enabled!(tracing::Level::INFO)
    } else {
        tracing::enabled!(tracing::Level::DEBUG)
    }
}
fn informative(kind: EventKind) -> bool {
    !matches!(
        kind,
        EventKind::AssistantMessage
            | EventKind::MessageAccepted
            | EventKind::ContinuationQueued
            | EventKind::TurnStarted
            | EventKind::TurnFinished
            | EventKind::ToolCompleted
            | EventKind::EvidenceSubmitted
            | EventKind::ThreadBound
            | EventKind::TerminalCleanupObserved
    )
}
fn provider_informative(kind: ProviderEventKind) -> bool {
    !matches!(
        kind,
        ProviderEventKind::ThreadMetadata
            | ProviderEventKind::ConnectionNotification
            | ProviderEventKind::RateLimitsUpdated
    )
}
impl Notice {
    fn emit(self) {
        match self {
            Self::Maintenance { gc: true, count } => {
                tracing::info!(name: "gc.completed", agents = count);
            }
            Self::Maintenance { gc: false, count } => {
                tracing::info!(name: "purge.completed", agents = count);
            }
            Self::Agent {
                id,
                execution,
                kind,
            } => {
                emit_agent(&id, execution.as_deref(), kind);
            }
            Self::Provider { id, kind } => emit_provider(&id, kind),
        }
    }
}

fn emit_agent(id: &str, execution: Option<&str>, kind: EventKind) {
    if emit_agent_lifecycle(id, execution, kind) {
        return;
    }
    match kind {
        EventKind::Spawned => {
            tracing::info!(name: "agent.spawned", agent_id = id, execution_id = execution);
        }
        EventKind::ApprovalRequested => {
            tracing::info!(name: "approval.requested", agent_id = id, execution_id = execution);
        }
        EventKind::ApprovalClosed => {
            tracing::info!(name: "approval.closed", agent_id = id, execution_id = execution);
        }
        EventKind::AuthSwitched => {
            tracing::info!(name: "auth.switch_applied", agent_id = id, execution_id = execution);
        }
        EventKind::AuthSwitchDeferred => {
            tracing::info!(name: "auth.switch_deferred", agent_id = id, execution_id = execution);
        }
        EventKind::MessagePromoted => {
            tracing::info!(name: "schedule.promoted", agent_id = id, execution_id = execution);
        }
        EventKind::MessageScheduleFailed => {
            tracing::info!(name: "schedule.failed", agent_id = id, execution_id = execution);
        }
        EventKind::JoinCompleted => {
            tracing::info!(name: "join.completed", agent_id = id, execution_id = execution);
        }
        EventKind::JoinDeadlinePassed => {
            tracing::info!(name: "join.deadline", agent_id = id, execution_id = execution);
        }
        EventKind::BranchRequested => {
            tracing::info!(name: "branch.fork", agent_id = id, execution_id = execution);
        }
        EventKind::BranchReady => {
            tracing::info!(name: "branch.ready", agent_id = id, execution_id = execution);
        }
        EventKind::BranchFailed => {
            tracing::info!(name: "branch.failed", agent_id = id, execution_id = execution);
        }
        EventKind::CollectorOverflow => {
            tracing::info!(name: "collector.overflow", agent_id = id, execution_id = execution);
        }
        _ if informative(kind) => {
            tracing::info!(name: "agent.transition",
                 agent_id = id,
                 execution_id = execution,
                 phase = kind.as_str());
        }
        _ => {
            tracing::debug!(name: "agent.event",
                 agent_id = id,
                 execution_id = execution,
                 phase = kind.as_str());
        }
    }
}
fn emit_provider(id: &str, kind: ProviderEventKind) {
    match kind {
        ProviderEventKind::Started => tracing::info!(name: "provider.started", provider_id = id),
        ProviderEventKind::Shutdown => tracing::info!(name: "provider.exit", provider_id = id),
        ProviderEventKind::LoginLimited => {
            tracing::info!(name: "auth.limit_observed", provider_id = id);
        }
        _ if provider_informative(kind) => {
            tracing::info!(name: "provider.transition", provider_id = id, phase = kind.as_str());
        }
        _ => tracing::debug!(name: "provider.event", provider_id = id, phase = kind.as_str()),
    }
}

fn emit_agent_lifecycle(id: &str, execution: Option<&str>, kind: EventKind) -> bool {
    match kind {
        EventKind::ExecutionStarting => {
            tracing::info!(name: "agent.execution_starting",
                 agent_id = id,
                 execution_id = execution);
        }
        EventKind::SessionReady => {
            tracing::info!(name: "agent.execution_started",
                 agent_id = id,
                 execution_id = execution);
        }
        EventKind::ExecutionStopped => {
            tracing::info!(name: "agent.execution_stopped",
                 agent_id = id,
                 execution_id = execution);
        }
        EventKind::ExecutionFailed => {
            tracing::info!(name: "agent.execution_failed", agent_id = id, execution_id = execution);
        }
        EventKind::SessionQuarantined => {
            tracing::info!(name: "agent.quarantined", agent_id = id, execution_id = execution);
        }
        EventKind::TaskCompleted => {
            tracing::info!(name: "agent.completed", agent_id = id, execution_id = execution);
        }
        EventKind::TerminationRequested => {
            tracing::info!(name: "agent.terminated", agent_id = id, execution_id = execution);
        }
        _ => return false,
    }
    true
}
