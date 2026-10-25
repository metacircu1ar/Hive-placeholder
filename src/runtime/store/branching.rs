//! Persisted branch intents, source holds and native binding transitions.
use crate::runtime::api::{self, events};
mod queries;

use super::{Queries, Store, handoff::Handoff, sql::FromRow as _};
use crate::{
    error::{Error, Result},
    runtime::{
        branching::{BranchFailure, BranchPhase, FinishedTurn, Intent, MATERIALIZATION_SECONDS},
        domain::{Agent, EventKind},
    },
    state,
};
use rusqlite::params;

// A branch handoff keeps the two latest rows of each section within 15,000 bytes.
const HANDOFF_SECTION_BYTES: usize = 15_000;
const HANDOFF_SECTION_ROWS: usize = 2;

struct SettledBoundary<'a> {
    id: &'a str,
    finished: &'a FinishedTurn,
    deadline: u64,
}

super::rows::record! {
    struct Receipt {
        status: crate::runtime::domain::MessageState = 0,
        thread: Option<String> = 1,
        turn: Option<String> = 2,
        execution: Option<String> = 3,
    }
}

fn materialization_deadline(at: u64) -> Result<u64> {
    let deadline = at
        .checked_add(MATERIALIZATION_SECONDS)
        .ok_or_else(super::numeric_range)?;
    crate::convert::sqlite_i64(deadline).map_err(super::numeric)?;
    Ok(deadline)
}

fn branch_live(child: &Agent, intent: &Intent) -> bool {
    !child.terminal()
        && !child.paused
        && child.control.is_none()
        && intent
            .deadline
            .is_some_and(|deadline| deadline > state::now())
}

fn busy() -> Error {
    Error::agent_busy(concat!(
        "The source has no confirmed fork boundary or its conversation is ",
        "changing. Retry after its current native input is acknowledged or ",
        "its session preparation finishes.",
    ))
}
impl Queries {
    pub(crate) fn fork_intent(&self, child: &Agent) -> Result<Option<Intent>> {
        match (child.branch_phase, self.branch_intent(&child.id)?) {
            (None, None) => Ok(None),
            (Some(BranchPhase::Bound), Some(intent)) if intent.phase == BranchPhase::Bound => {
                Ok(None)
            }
            (Some(BranchPhase::ReadyToFork), Some(intent))
                if intent.phase == BranchPhase::ReadyToFork =>
            {
                Ok(Some(intent))
            }
            (Some(phase), Some(intent)) if intent.phase == phase => Err(Error::agent_busy(
                "The branch is no longer ready for native materialization.",
            )),
            _ => Err(Error::state_inconsistent(
                "Agent branch gate and native capture record disagree.",
            )),
        }
    }
    // Called while the source hold excludes later native submissions. Only receipts
    // for this conversation enter the summary; queued input and other chats do not.
    fn branch_handoff(&self, intent: &Intent) -> Result<String> {
        let mut out = Handoff::new(format!(
            concat!(
                "Inherited context summary from agent {}, conversation {}, through ",
                "turn {}. Historical context only.\n",
            ),
            intent.source, intent.source_thread, intent.source_turn
        ));
        for sql in [queries::HANDOFF_INPUTS, queries::HANDOFF_EVENTS] {
            self.sql().cached().with_rows(
                sql,
                params![intent.source, intent.source_thread],
                |rows| out.rows(rows, HANDOFF_SECTION_BYTES, HANDOFF_SECTION_ROWS),
            )?;
        }
        Ok(out.finish())
    }
    pub(crate) fn branch_dependents(&self, id: &str) -> Result<Vec<String>> {
        self.sql()
            .cached()
            .query_column(queries::BRANCH_DEPENDENTS, [id])
    }
    pub(crate) fn branch_intent(&self, id: &str) -> Result<Option<Intent>> {
        self.query_opt_cached(queries::BRANCH_INTENT, [id], |r| {
            let phase = r.get(4)?;
            Ok(Intent {
                source: r.get(0)?,
                source_thread: r.get(1)?,
                source_execution: r.get(2)?,
                source_turn: r.get(3)?,
                phase,
                deadline: super::rows::optional_unsigned(r, 5)?,
                source_outcome: r.get(6)?,
                source_handoff: r.get(7)?,
                native_thread: r.get(8)?,
                error: r.get(9)?,
            })
        })
    }

    pub(crate) fn branch_boundary(&self, agent: &Agent) -> Result<Intent> {
        if agent.branch_unbound()
            || agent.auth_switch_pending.is_some()
            || agent.recovery_fence.is_some()
        {
            return Err(busy());
        }
        let thread = agent.thread_id.as_deref().ok_or_else(busy)?;
        let execution = agent.execution_id.as_deref().ok_or_else(busy)?;
        let submitting: bool =
            self.query_row_cached(queries::BOUNDARY_UNACKNOWLEDGED_INPUT, [&agent.id], |r| {
                r.get(0)
            })?;
        if submitting {
            return Err(busy());
        }
        let (turn, deadline, outcome) = if let Some(turn) = agent.turn_id.as_ref() {
            (turn.clone(), None, None)
        } else {
            let claimed: bool = self.claimed(&agent.id)?;
            if claimed && agent.state == crate::runtime::domain::AgentState::Starting {
                return Err(busy());
            }
            let finished = agent
                .last_finished_turn
                .as_ref()
                .filter(|f| f.thread == thread)
                .ok_or_else(busy)?;
            self.confirm_latest_attempt(agent, finished)?;
            (
                finished.turn.clone(),
                Some(materialization_deadline(state::now())?),
                Some(finished.outcome),
            )
        };
        Ok(Intent {
            source: agent.id.clone(),
            source_thread: thread.into(),
            source_execution: execution.into(),
            source_turn: turn,
            phase: BranchPhase::WaitingForSource,
            deadline,
            source_outcome: outcome,
            source_handoff: None,
            native_thread: None,
            error: None,
        })
    }

    fn confirm_latest_attempt(&self, agent: &Agent, finished: &FinishedTurn) -> Result<()> {
        let Some(attempt) = &agent.last_attempted_turn else {
            return if finished.message.is_none() {
                Ok(())
            } else {
                Err(busy())
            };
        };
        let receipt: Option<Receipt> = self.query_opt_cached(
            queries::LATEST_ATTEMPT_RECEIPT,
            params![agent.id, attempt.message],
            Receipt::read,
        )?;
        let Some(Receipt {
            status,
            thread,
            turn,
            execution,
        }) = receipt
        else {
            return Err(busy());
        };
        // A submission returned to accepted only with the existing proven-unsent fence.
        if status == crate::runtime::domain::MessageState::Accepted {
            return Ok(());
        }
        if !matches!(
            status,
            crate::runtime::domain::MessageState::Completed
                | crate::runtime::domain::MessageState::Failed
                | crate::runtime::domain::MessageState::Interrupted
        ) || finished.message.as_deref() != Some(&attempt.message)
            || attempt.thread != finished.thread
            || attempt.execution != finished.execution
            || thread.as_deref() != Some(&finished.thread)
            || turn.as_deref() != Some(&finished.turn)
            || execution.as_deref() != Some(&finished.execution)
        {
            return Err(busy());
        }

        Ok(())
    }
}
impl Store {
    /// Bounded settlement runs on the existing deadline owner, never provider I/O.
    pub(crate) fn settle_branches(&self, now: u64) -> Result<usize> {
        let ids: Vec<String> = self.sql().cached().query_column(
            queries::BRANCHES_TO_SETTLE,
            [crate::convert::sqlite_i64(now).map_err(super::numeric)?],
        )?;
        for id in &ids {
            self.settle_branch(id, now)?;
        }
        Ok(ids.len())
    }

    fn settle_branch(&self, id: &str, now: u64) -> Result<()> {
        let mut child = self.agent(id)?;
        let intent = self.branch_intent(id)?.ok_or_else(busy)?;
        if intent.deadline.is_some_and(|at| at <= now) {
            self.fail_branch(
                &mut child,
                BranchFailure::BranchMaterializationTimeout,
                true,
            )?;
            return Ok(());
        }
        if intent.phase == BranchPhase::Forking {
            self.fail_branch(&mut child, BranchFailure::BranchCreationUncertain, false)?;
            return Ok(());
        }
        let source = self.agent(&intent.source)?;
        if child.terminal() || child.paused || child.control.is_some() {
            self.fail_branch(&mut child, BranchFailure::BranchCancelled, false)?;
            return Ok(());
        }
        let boundary = source
            .last_finished_turn
            .as_ref()
            .filter(|f| f.thread == intent.source_thread && f.turn == intent.source_turn);
        let Some(finished) = boundary else {
            self.fail_branch(&mut child, BranchFailure::SourceBoundaryUncertain, false)?;
            return Ok(());
        };
        if source.thread_id.as_deref() != Some(&intent.source_thread)
            || source.auth_generation != child.auth_generation
            || source.auth != child.auth
        {
            self.fail_branch(&mut child, BranchFailure::SourceIdentityChanged, false)?;
            return Ok(());
        }
        let deadline = intent
            .deadline
            .map_or_else(|| materialization_deadline(finished.at), Ok)?;
        if deadline <= now {
            self.fail_branch(
                &mut child,
                BranchFailure::BranchMaterializationTimeout,
                true,
            )?;
            return Ok(());
        }
        self.publish_branch_settlement(
            &mut child,
            &intent,
            &source,
            SettledBoundary {
                id,
                finished,
                deadline,
            },
        )
    }

    fn publish_branch_settlement(
        &self,
        child: &mut Agent,
        intent: &Intent,
        source: &Agent,
        boundary: SettledBoundary<'_>,
    ) -> Result<()> {
        let SettledBoundary {
            id,
            finished,
            deadline,
        } = boundary;
        let claimed: bool = self.claimed(&source.id)?;
        if !claimed && !source.conversation_clean {
            self.fail_branch(child, BranchFailure::SourceCleanupUncertain, false)?;
            return Ok(());
        }
        let handoff = self.branch_handoff(intent)?;
        let phase = if claimed {
            BranchPhase::WaitingForSource
        } else {
            BranchPhase::ReadyToFork
        };
        self.execute_cached(
            queries::SETTLE_BRANCH,
            params![
                id,
                crate::convert::sqlite_i64(deadline).map_err(super::numeric)?,
                finished.outcome,
                handoff,
                phase.as_str()
            ],
        )?;
        self.changed(id);
        self.deadlines_changed();
        Ok(())
    }

    pub(crate) fn fail_branch(
        &self,
        child: &mut Agent,
        reason: BranchFailure,
        release: bool,
    ) -> Result<()> {
        let Some(intent) = self.branch_intent(&child.id)? else {
            return Ok(());
        };
        if intent.phase == BranchPhase::Bound {
            return Ok(());
        }
        let uncertain = matches!(intent.phase, BranchPhase::Forking | BranchPhase::Uncertain);
        let phase = if uncertain {
            BranchPhase::Uncertain
        } else {
            BranchPhase::Failed
        };
        let claimed: bool = self.claimed(&child.id)?;
        let hold = !release && (claimed || uncertain && !child.conversation_clean);
        // The stored error is a public diagnostic aggregation; project typed reasons once.
        let reason = reason.as_str();
        let error = match intent.error.as_deref() {
            Some(first) if !first.split("; ").any(|previous| previous == reason) => {
                format!("{first}; {reason}")
            }
            Some(first) => first.to_owned(),
            None => reason.to_owned(),
        };
        self.execute_cached(
            queries::FAIL_BRANCH,
            params![child.id, phase.as_str(), error, hold],
        )?;
        child.branch_phase = Some(phase);
        child.error = Some(error);
        child.control = claimed.then_some(crate::runtime::domain::Control::Stop);
        if !child.terminal() {
            child.state = crate::runtime::domain::AgentState::Failed;
        }
        if uncertain {
            child.recovery_fence =
                Some(crate::runtime::domain::RecoveryFence::BranchCreationUncertain);
        }
        self.save(child)?;
        self.undeliverable(&child.id)?;
        if !matches!(intent.phase, BranchPhase::Failed | BranchPhase::Uncertain) {
            self.event(
                child,
                EventKind::BranchFailed,
                api::value(events::BranchFailed {
                    source: &intent.source,
                    reason,
                    uncertain,
                }),
            )?;
        }
        self.changed(&intent.source);
        self.deadlines_changed();
        Ok(())
    }

    pub(crate) fn branch_execution_released(&self, id: &str) -> Result<()> {
        if let Some(intent) = self.branch_intent(id)?
            && matches!(intent.phase, BranchPhase::Failed | BranchPhase::Uncertain)
            && (intent.phase == BranchPhase::Failed || self.agent(id)?.conversation_clean)
        {
            self.execute_cached(queries::RELEASE_BRANCH_HOLD, [id])?;
            self.changed(&intent.source);
        }
        self.deadlines_changed();
        Ok(())
    }

    pub(crate) fn begin_branch(&self, child: &mut Agent) -> Result<Option<Intent>> {
        let Some(intent) = self.fork_intent(child)? else {
            return Ok(None);
        };
        if intent.phase != BranchPhase::ReadyToFork || !branch_live(child, &intent) {
            return Err(Error::agent_busy(
                "The branch is not ready for native materialization.",
            ));
        }
        self.execute_cached(queries::BEGIN_BRANCH, [&child.id])?;
        child.branch_phase = Some(BranchPhase::Forking);
        Ok(Some(intent))
    }

    /// Only call under the current execution fence with explicit pre-creation
    /// rejection evidence. A timeout or an I/O error cannot restore retryability.
    pub(crate) fn branch_creation_rejected(
        &self,
        child: &mut Agent,
        reason: BranchFailure,
    ) -> Result<bool> {
        let intent = self.branch_intent(&child.id)?.ok_or_else(busy)?;
        if !matches!(intent.phase, BranchPhase::Forking | BranchPhase::Uncertain)
            || intent.native_thread.is_some()
        {
            return Err(busy());
        }
        self.execute_cached(
            queries::REJECT_BRANCH_CREATION,
            params![child.id, reason.as_str()],
        )?;
        child.branch_phase = Some(BranchPhase::ReadyToFork);
        if child.recovery_fence
            == Some(crate::runtime::domain::RecoveryFence::BranchCreationUncertain)
        {
            child.recovery_fence = None;
        }
        if !branch_live(child, &intent) {
            self.fail_branch(child, BranchFailure::BranchCancelledBeforeCreation, false)?;
            return Ok(false);
        }
        self.save(child)?;
        Ok(true)
    }

    pub(crate) fn record_branch_native(&self, child: &Agent, thread: &str) -> Result<()> {
        self.execute_cached(queries::RECORD_BRANCH_NATIVE, params![child.id, thread])?;
        Ok(())
    }

    /// Native acknowledgement must be owned by this still-live materialization attempt.
    pub(crate) fn bind_branch(&self, child: &mut Agent, thread: &str) -> Result<()> {
        let Some(intent) = self.branch_intent(&child.id)? else {
            return Ok(());
        };
        if intent.phase == BranchPhase::Bound {
            return Ok(());
        }
        if intent.phase != BranchPhase::Forking
            || !branch_live(child, &intent)
            || intent.source_thread == thread
        {
            return Err(Error::stale_execution(
                "The native fork acknowledgement no longer owns a live branch.",
            ));
        }
        self.execute_cached(queries::BIND_BRANCH, params![child.id, thread])?;
        child.branch_phase = Some(BranchPhase::Bound);
        child.thread_id = Some(thread.into());
        child.last_finished_turn = Some(FinishedTurn {
            thread: thread.into(),
            execution: child.execution_id.clone().ok_or_else(busy)?,
            turn: intent.source_turn.clone(),
            message: None,
            outcome: intent
                .source_outcome
                .unwrap_or(crate::runtime::domain::ProviderTurnStatus::Completed),
            at: state::now(),
        });
        self.save(child)?;
        self.event(
            child,
            EventKind::BranchReady,
            api::value(events::BranchReady {
                source: &intent.source,
                source_turn: &intent.source_turn,
                thread_id: thread,
            }),
        )?;
        self.changed(&intent.source);
        self.deadlines_changed();
        Ok(())
    }

    pub(crate) fn install_branch(&self, child: &Agent, intent: &Intent) -> Result<()> {
        let deadline = intent
            .deadline
            .map(crate::convert::sqlite_i64)
            .transpose()
            .map_err(super::numeric)?;
        self.execute_cached(
            queries::INSTALL_BRANCH,
            params![
                child.id,
                intent.source,
                intent.source_thread,
                intent.source_execution,
                intent.source_turn,
                intent.phase.as_str(),
                deadline,
                intent.source_outcome
            ],
        )?;
        self.execute_cached(
            queries::INHERIT_BRANCH_PRIORITY,
            params![child.id, intent.source],
        )?;
        self.inherit_image_usage(&child.id, &intent.source)?;
        self.event(child, EventKind::BranchRequested, api::value(intent))?;
        self.changed(&intent.source);
        self.changed(&child.id);
        self.deadlines_changed();
        Ok(())
    }
}

#[cfg(test)]
#[test]
fn materialization_deadlines_are_checked_before_addition_and_storage() {
    let maximum = i64::MAX.unsigned_abs();
    assert_eq!(
        materialization_deadline(maximum - MATERIALIZATION_SECONDS)
            .expect("representable deadline"),
        maximum
    );
    for at in [maximum - MATERIALIZATION_SECONDS + 1, maximum, u64::MAX] {
        assert_eq!(
            materialization_deadline(at)
                .expect_err("out-of-range deadline")
                .code(),
            "database_error"
        );
    }
}
