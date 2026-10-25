//! Retained replies for external sends and joined members. Other internal turns skip these
//! writes.
pub(super) mod queries;
use super::{Agent, EventKind, Queries, Store, api, rows, sql::FromRow as _};
use crate::{
    error::{Error, Result},
    runtime::domain::{DeliveryMode, MessageState},
};
use rusqlite::params;
use serde_json::Value;

// The queue-count SQL reads one extra row; report values above 10,000 as inexact.
const QUEUE_COUNT_LIMIT: u64 = 10_000;

crate::runtime::domain::wire_enum!(AssistantPhase { FinalAnswer => "final_answer" });

impl Store {
    pub fn track_call(&self, id: &str) -> Result<()> {
        self.execute_cached(queries::TRACK_CALL, params![id, super::convert::now()?])?;
        Ok(())
    }

    pub fn assistant_output(
        &self,
        agent: &Agent,
        message: Option<&str>,
        data: Value,
        model_text: bool,
    ) -> Result<()> {
        let turn = data["turn_id"].as_str().map(str::to_owned);
        let final_answer =
            data["phase"].as_str().map(AssistantPhase::parse) == Some(AssistantPhase::FinalAnswer);
        let text = data["text"].is_string();
        let join_text = (self.joins_possible.get() && model_text && message.is_some())
            .then(|| data["text"].as_str().map(str::to_owned))
            .flatten();
        let stored = self.record_event(agent, EventKind::AssistantMessage, data)?;
        if let Some(message) = message.filter(|_| model_text && text) {
            let updated = self.execute_cached(
                queries::ASSISTANT_OUTPUT,
                params![
                    stored,
                    final_answer,
                    message,
                    agent.execution_id,
                    turn,
                    agent.id
                ],
            )?;
            if updated == 1
                && let Some(text) = join_text
            {
                self.join_output(message, &text, final_answer)?;
            }
        }
        Ok(())
    }
}

#[allow(clippy::struct_excessive_bools)] // Independent persisted flags, validated at the boundary.
struct Saved {
    result: api::MessageResult,
    sequence: i64,
    startup: bool,
    redirect: bool,
    mode: DeliveryMode,
    expired: bool,
    tracked: bool,
    delivery_present: bool,
    schedule_integrity: Option<String>,
    output: Option<String>,
}

impl super::sql::FromRow for Saved {
    fn read(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        let state: MessageState = r.get(1)?;
        Ok(Saved {
            result: api::MessageResult {
                held_for_build: None,
                message_id: String::new(),
                not_before: rows::optional_unsigned(r, 18)?,
                recipient: r.get(0)?,
                delivery_state: state,
                outcome: outcome(state),
                execution_id: r.get(6)?,
                thread_id: r.get(7)?,
                turn_id: r.get(8)?,
                delivery_error: r.get(9)?,
                events_after: rows::optional_unsigned(r, 12)?.map(|n| n.saturating_sub(1)),
                output: None,
                output_selection: if r.get::<_, Option<String>>(13)?.is_none() {
                    api::OutputSelection::NoText
                } else if r.get::<_, Option<bool>>(17)?.unwrap_or(false) {
                    api::OutputSelection::FinalAnswer
                } else {
                    api::OutputSelection::LastAssistantMessage
                },
                output_partial: state != MessageState::Completed,
                queued_ahead: None,
                queue_count_exact: state != MessageState::Scheduled,
                recipient_state: r.get(14)?,
                paused: r.get(15)?,
                active_input: r.get(16)?,
            },
            sequence: rows::nonnegative(r, 2)?,
            startup: r.get(3)?,
            redirect: r.get(4)?,
            mode: r.get(5)?,
            expired: r.get::<_, Option<bool>>(10)?.unwrap_or(false),
            delivery_present: r.get::<_, Option<String>>(20)?.is_some(),
            schedule_integrity: r.get(19)?,
            tracked: r.get::<_, Option<String>>(11)?.is_some(),
            output: r.get(13)?,
        })
    }
}

impl Queries {
    pub fn message_result(&self, id: &str) -> Result<api::MessageResult> {
        let mut saved = self
            .query_opt_cached(queries::MESSAGE_RESULT, [id], Saved::read)?
            .ok_or_else(|| Error::unknown_message("No external message has this ID."))?;
        id.clone_into(&mut saved.result.message_id);
        self.finish_result(saved)
    }

    fn finish_result(&self, saved: Saved) -> Result<api::MessageResult> {
        if saved.schedule_integrity.is_some()
            || !saved.delivery_present
            || (saved.result.delivery_state == MessageState::Scheduled
                && !self
                    .scheduled_proof(&saved.result.message_id, "human")?
                    .valid)
        {
            return Err(Error::database_error(
                "Missing or inconsistent message delivery proof.",
            ));
        }
        if saved.startup || saved.mode != DeliveryMode::NextTurn {
            return Err(Error::unsupported_operation(
                "Only external next-turn and redirect sends have call results.",
            ));
        }
        if saved.expired {
            return Err(Error::result_expired(
                "Compaction expired this message's retained output.",
            ));
        }
        if !saved.tracked {
            return Err(Error::database_error(
                "Missing retained external message result.",
            ));
        }
        let mut result = saved.result;
        result.output = saved
            .output
            .map(|s| {
                serde_json::from_str(&s).map_err(|e| {
                    Error::database_error("Invalid saved message output.").with_source(e)
                })
            })
            .transpose()?;
        if result.delivery_state == MessageState::Accepted {
            // The partial mailbox index bounds both matching rows and rank work.
            let ahead: u64 = self.query_row_cached(
                queries::QUEUE_RANK,
                params![
                    result.recipient,
                    saved.startup,
                    saved.redirect,
                    saved.sequence
                ],
                |r| rows::unsigned(r, 0),
            )?;
            result.queued_ahead = Some(ahead.min(QUEUE_COUNT_LIMIT));
            result.queue_count_exact = ahead <= QUEUE_COUNT_LIMIT;
        }
        if result.outcome == api::CallOutcome::Pending {
            result.held_for_build = self.recipient_hold(&result.recipient)?;
            if result.held_for_build.is_some() {
                result.outcome = api::CallOutcome::Held;
            }
        }
        Ok(result)
    }
}

fn outcome(state: MessageState) -> api::CallOutcome {
    use api::CallOutcome;
    match state {
        MessageState::Scheduled
        | MessageState::Accepted
        | MessageState::Submitting
        | MessageState::Submitted => CallOutcome::Pending,
        MessageState::Completed => CallOutcome::Replied,
        MessageState::Interrupted | MessageState::Undeliverable | MessageState::StaleTurn => {
            CallOutcome::Cancelled
        }
        MessageState::Failed => CallOutcome::Failed,
        MessageState::Uncertain => CallOutcome::Uncertain,
    }
}

// Three disjoint index ranges bound work even when this message is first.
