//! Approval history uses the same transaction and reader snapshots as messaging.
use crate::runtime::api::{self, events};
mod queries;
use super::convert::{optional_timestamp, timestamp};
use super::{Queries, Store};
use crate::{
    approvals::{Approval, ApprovalAction, ApprovalState, ApprovalSummary, ApprovalsPage},
    error::{Error, Result},
    runtime::domain::{Agent, EventKind},
    state,
};
use rusqlite::{Row, params, types::Type};
use serde_json::Value;

impl Queries {
    pub(crate) fn pending_approval(&self, agent: &str) -> Result<Option<String>> {
        self.query_opt_cached(queries::PENDING_APPROVAL, [agent], |r| r.get(0))
    }

    pub(crate) fn approval(&self, id: &str) -> Result<Approval> {
        self.query_opt_cached(queries::APPROVAL, [id], approval_row)?
            .ok_or_else(|| Error::unknown_approval("No approval has this ID."))
    }

    pub(crate) fn approvals(&self, args: &api::ApprovalsArgs) -> Result<ApprovalsPage> {
        let api::ApprovalOptions {
            agent,
            all,
            after,
            limit,
        } = args.options()?;
        if let Some(agent) = agent {
            self.agent(agent)?;
        }
        let sql = queries::approvals(agent.is_some(), all);
        let now = state::now();
        self.sql().cached().with_rows(
            &sql,
            params![
                agent,
                after.map_or(0, |c| crate::convert::capped_i64(c.created_at)),
                after.map_or("", |c| c.id.as_str()),
                crate::convert::capped_i64(limit)
            ],
            |rows| {
                let mut approvals = Vec::new();
                let mut budget = super::PageBudget::new();
                while let Some(row) = rows.next_with(|r| {
                    let created_at = timestamp(r, 7)?;
                    Ok(ApprovalSummary {
                        id: r.get(0)?,
                        agent_id: r.get(1)?,
                        role: r.get(2)?,
                        agent_name: r.get(9)?,
                        title: r.get(3)?,
                        destructive: r.get(4)?,
                        state: r.get(5)?,
                        action: r.get(6)?,
                        created_at,
                        closed_at: optional_timestamp(r, 8)?,
                        age_seconds: now.saturating_sub(created_at),
                    })
                })? {
                    let size = api::value(&row).to_string().len();
                    if !budget.accept(size)? {
                        break;
                    }
                    approvals.push(row);
                }
                let next_after = approvals.last().map(|a| api::ApprovalCursor {
                    created_at: a.created_at,
                    id: a.id.clone(),
                });
                Ok(ApprovalsPage {
                    approvals,
                    next_after,
                })
            },
        )
    }
}

impl Store {
    pub(crate) fn insert_approval(&self, approval: &Approval) -> Result<()> {
        self.execute_cached(
            queries::INSERT_APPROVAL,
            params![
                approval.id,
                approval.agent_id,
                approval.execution,
                approval.turn,
                approval.title,
                approval.message,
                approval.schema.as_ref().map(Value::to_string),
                approval.destructive,
                sql_timestamp(approval.created_at)?
            ],
        )?;
        Ok(())
    }

    pub(crate) fn close_pending_approval(&self, agent: &Agent, state: ApprovalState) -> Result<()> {
        debug_assert!(matches!(
            state,
            ApprovalState::Superseded | ApprovalState::Withdrawn
        ));
        let Some(id) = self.pending_approval(&agent.id)? else {
            return Ok(());
        };
        self.execute_cached(
            queries::CLOSE_PENDING_APPROVAL,
            params![id, state.as_str(), sql_timestamp(state::now())?],
        )?;
        self.event(
            agent,
            EventKind::ApprovalClosed,
            api::value(events::ApprovalClosed {
                origin: None,
                approval_id: &id,
                state,
                action: None,
                response_message: None,
            }),
        )?;
        Ok(())
    }

    pub(crate) fn answer_approval(
        &self,
        id: &str,
        action: ApprovalAction,
        content: Option<&Value>,
        reason: Option<&str>,
        message: &str,
    ) -> Result<()> {
        let changed = self.execute_cached(
            queries::ANSWER_APPROVAL,
            params![
                id,
                action.as_str(),
                content.map(Value::to_string),
                reason,
                sql_timestamp(state::now())?,
                message
            ],
        )?;
        if changed != 1 {
            return Err(Error::approval_closed(
                "This approval is no longer pending.",
            ));
        }
        Ok(())
    }

    pub(crate) fn enqueue_approval_answer(&self, id: &str, agent: &str, body: &str) -> Result<()> {
        self.enqueue_ordered(id, "human", agent, body, false, true)?;
        self.execute_cached(queries::ENQUEUE_APPROVAL_ANSWER, [id])?;
        self.track_call(id)
    }
}

fn json_column(row: &Row<'_>, index: usize) -> rusqlite::Result<Option<Value>> {
    let value: Option<String> = row.get(index)?;
    value
        .map(|v| {
            serde_json::from_str(&v).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(e))
            })
        })
        .transpose()
}
fn approval_row(row: &Row<'_>) -> rusqlite::Result<Approval> {
    Ok(Approval {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        execution: row.get(2)?,
        turn: row.get(3)?,
        title: row.get(4)?,
        message: row.get(5)?,
        schema: json_column(row, 6)?,
        destructive: row.get(7)?,
        state: row.get(8)?,
        action: row.get(9)?,
        content: json_column(row, 10)?,
        reason: row.get(11)?,
        created_at: timestamp(row, 12)?,
        closed_at: optional_timestamp(row, 13)?,
        response_message: row.get(14)?,
    })
}

fn sql_timestamp(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|cause| {
        Error::database_error("Approval timestamp exceeds SQLite range.").with_source(cause)
    })
}
