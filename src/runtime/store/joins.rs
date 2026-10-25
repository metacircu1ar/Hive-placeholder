//! Durable fan-in settlement. All mailbox publication stays in the writer transaction.
mod output;
mod queries;
use super::{Queries, Store, api, convert::stamp, rows};
use crate::{
    error::{Error, Result},
    runtime::{
        api::joins::{JoinMember, JoinOutcome, JoinOutput, JoinSnapshot, JoinState},
        domain::EventKind,
    },
};
use rusqlite::params;
use std::fmt::Write as _;

impl Queries {
    pub(crate) fn join_snapshot(&self, id: &str) -> Result<JoinSnapshot> {
        self.find_join_snapshot(id)?
            .ok_or_else(|| Error::invalid_request("No retained join has this ID."))
    }

    pub(crate) fn find_join_snapshot(&self, id: &str) -> Result<Option<JoinSnapshot>> {
        let Some(mut value) = self.query_opt_cached(queries::SNAPSHOT, [id], |r| {
            Ok(JoinSnapshot {
                held_for_build: None,
                join_id: r.get(0)?,
                requester: r.get(1)?,
                request_id: r.get(2)?,
                created_at: rows::unsigned(r, 3)?,
                deadline: rows::optional_unsigned(r, 4)?,
                state: r.get(5)?,
                members: Vec::new(),
            })
        })?
        else {
            return Ok(None);
        };
        let members = self.sql().cached().query_map(queries::MEMBERS, [id], |r| {
            let output: Option<String> = r.get(4)?;
            let outcome: JoinOutcome = r.get(3)?;
            Ok((
                JoinMember {
                    held_for_build: None,
                    position: rows::count(r, 0)?,
                    message_id: r.get(1)?,
                    target: r.get(2)?,
                    outcome,
                    output: None,
                    output_selection: if output.is_none() {
                        api::OutputSelection::NoText
                    } else if r.get::<_, bool>(5)? {
                        api::OutputSelection::FinalAnswer
                    } else {
                        api::OutputSelection::LastAssistantMessage
                    },
                    output_partial: outcome != JoinOutcome::Completed,
                    task_completed: None,
                    delivered: r.get(7)?,
                },
                output,
                r.get::<_, Option<String>>(6)?,
            ))
        })?;
        for (mut member, output, completion) in members {
            member.output = decode_output(output)?;
            member.task_completed = decode_output(completion)?;
            if member.outcome == JoinOutcome::Pending {
                member.held_for_build = self.recipient_hold(&member.target)?;
            }
            value.members.push(member);
        }
        if !value.closed() {
            value.held_for_build = self
                .recipient_hold(&value.requester)?
                .or_else(|| value.members.iter().find_map(|m| m.held_for_build));
        }
        Ok(Some(value))
    }
    pub(crate) fn open_joins(&self, requester: &str) -> Result<Vec<String>> {
        self.sql().cached().query_column(queries::OPEN, [requester])
    }
    /// Whether completing `requester` now would cancel a join it still waits
    /// for, before any deadline, or discard an undelivered join result.
    pub(crate) fn join_blocks_completion(&self, requester: &str) -> Result<bool> {
        self.query_row_cached(queries::BLOCKS_COMPLETION, [requester], |r| r.get(0))
    }
    pub(crate) fn next_join_deadline(&self) -> Result<Option<u64>> {
        self.query_row_cached(queries::NEXT_DEADLINE, [], |r| {
            rows::optional_unsigned(r, 0)
        })
    }
    pub(crate) fn request_used(&self, sender: &str, id: &str) -> Result<bool> {
        self.query_row_cached(queries::REQUEST_USED, params![sender, id], |r| r.get(0))
    }
}
fn decode_output(raw: Option<String>) -> Result<Option<JoinOutput>> {
    raw.map(|text| {
        crate::runtime::domain::decode::from_str(&text).map_err(|cause| {
            Error::database_error("Invalid retained join output.").with_source(cause)
        })
    })
    .transpose()
}
impl Store {
    pub(crate) fn create_join(
        &self,
        requester: &str,
        request: &str,
        deadline: Option<u64>,
    ) -> Result<String> {
        // Never undo this cache on rollback. A false positive only costs a lookup.
        self.joins_possible.set(true);
        let id = uuid::Uuid::new_v4().to_string();
        self.execute_cached(
            queries::CREATE,
            params![
                id,
                requester,
                request,
                super::convert::now()?,
                deadline.map(stamp).transpose()?
            ],
        )?;
        self.deadlines_changed();
        Ok(id)
    }
    pub(crate) fn join_member(
        &self,
        join: &str,
        requester: &str,
        message: &str,
        position: usize,
    ) -> Result<()> {
        // Register immediately after each fresh operation, before later operations can end it.
        let changed = self.execute_cached(
            queries::ADD_MEMBER,
            params![
                join,
                requester,
                i64::try_from(position).map_err(super::numeric)?,
                message
            ],
        )?;
        if changed != 1 {
            return Err(Error::invalid_request(
                "Join member is missing or already joined.",
            ));
        }
        self.execute_cached(queries::ATTACH, params![join, message])?;
        self.execute_cached(queries::TRACK, params![message, super::convert::now()?])?;
        Ok(())
    }
    pub(crate) fn join_requested(
        &self,
        id: &str,
        requester: &str,
        request: &str,
        members: usize,
    ) -> Result<()> {
        self.join_event(
            requester,
            EventKind::JoinRequested,
            api::events::JoinEvent {
                join_id: id,
                request_id: request,
                members,
                state: JoinState::Collecting,
            },
        )
    }
    fn join_event(
        &self,
        requester: &str,
        kind: EventKind,
        data: api::events::JoinEvent<'_>,
    ) -> Result<()> {
        self.execute_cached(
            super::queries::INSERT_EVENT,
            params![
                requester,
                Option::<String>::None,
                super::convert::now()?,
                kind,
                api::value(data).to_string()
            ],
        )?;
        Ok(())
    }
    /// Called before readiness is prepared and before standalone outer commits.
    pub(crate) fn settle_joins(&self) -> Result<()> {
        if !self.joins_possible.get() {
            return Ok(());
        }
        loop {
            let ids: Vec<String> = self.sql().cached().query_column(queries::DIRTY, [])?;
            if ids.is_empty() {
                return Ok(());
            }
            for id in ids {
                self.settle_join(&id, false)?;
            }
        }
    }
    pub(crate) fn expire_join_deadlines(&self, now: u64) -> Result<usize> {
        if !self.joins_possible.get() {
            return Ok(0);
        }
        let ids: Vec<String> = self
            .sql()
            .cached()
            .query_column(queries::DUE, [stamp(now)?])?;
        for id in &ids {
            self.settle_join(id, true)?;
        }
        Ok(ids.len())
    }
    fn settle_join(&self, id: &str, deadline_due: bool) -> Result<()> {
        let (requester, request_id, state, members, pending) =
            self.query_row_cached(queries::SUMMARY, [id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, JoinState>(2)?,
                    rows::count(r, 3)?,
                    rows::count(r, 4)?,
                ))
            })?;
        let mut event = api::events::JoinEvent {
            join_id: id,
            request_id: &request_id,
            members,
            state,
        };
        self.execute_cached(queries::CLEAR_DIRTY, [id])?;
        if matches!(state, JoinState::Completed | JoinState::Cancelled) {
            return Ok(());
        }
        if requester != "human"
            && self
                .agent_or_purged(&requester)?
                .as_ref()
                .is_none_or(super::Agent::terminal)
        {
            event.state = JoinState::Cancelled;
            self.execute_cached(queries::STATE, params![event.state, id])?;
            return self.join_event(&requester, EventKind::JoinCancelled, event);
        }
        let completed = pending == 0;
        if !(completed || deadline_due && state == JoinState::Collecting) {
            return Ok(());
        }
        event.state = if completed {
            JoinState::Completed
        } else {
            JoinState::DeadlinePassed
        };
        if requester != "human" {
            // Large output JSON is only decoded when publishing an actual joined input.
            let mut snapshot = self.join_snapshot(id)?;
            snapshot.state = event.state;
            let body = render(&snapshot);
            let message = uuid::Uuid::new_v4().to_string();
            self.enqueue(&message, &format!("join:{id}"), &requester, &body)?;
            self.execute_cached(queries::RESULT_KIND, [&message])?;
            self.execute_cached(queries::DELIVERED, [id])?;
        }
        self.execute_cached(queries::STATE, params![event.state, id])?;
        self.join_event(
            &requester,
            if completed {
                EventKind::JoinCompleted
            } else {
                EventKind::JoinDeadlinePassed
            },
            event,
        )?;
        self.deadlines_changed();
        Ok(())
    }
}
fn render(join: &JoinSnapshot) -> String {
    let finished = join
        .members
        .iter()
        .filter(|m| m.outcome != JoinOutcome::Pending)
        .count();
    let mut body = format!(
        "Joined results for {} (join:{}, {}/{} finished, {}):\n",
        join.request_id,
        join.join_id,
        finished,
        join.members.len(),
        join.state.as_str()
    );
    for member in join.members.iter().filter(|m| !m.delivered) {
        let _ = writeln!(
            body,
            "--- {}. {}: {}{} [message {}] ---",
            member.position + 1,
            member.target,
            member.outcome.as_str(),
            if member.output_partial && member.output.is_some() {
                " (partial)"
            } else {
                ""
            },
            member.message_id
        );
        if member.outcome == JoinOutcome::Pending {
            continue;
        }
        for (label, output) in [
            ("", &member.output),
            ("Task completed: ", &member.task_completed),
        ] {
            if let Some(output) = output {
                body.push_str(label);
                body.push_str(&output.text);
                body.push('\n');
                if let Some(target) = &output.read_target {
                    let _ = writeln!(
                        body,
                        "[Truncated; read full output with hive_read target={target}]"
                    );
                }
            }
        }
    }
    body
}
