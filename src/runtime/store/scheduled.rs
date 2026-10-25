//! Durable next-turn timers. Only promotion enters the ordinary ready mailbox.
use crate::runtime::api::{self, events};
pub(super) mod queries;
use super::{Queries, Store};
use crate::{
    error::{Error, Result},
    runtime::domain::{EventKind, MessageState},
};
use rusqlite::params;

super::rows::record! {
    pub(in crate::runtime) struct Scheduled {
        pub seq: i64 = 0,
        pub id: String = 1,
        pub sender: String = 2,
        pub recipient: String = 3,
        pub body: String = 4,
        pub at: i64 = 5,
        pub join_id: Option<String> = 6,
        pub has_attachments: bool = 7,
    }
}

impl Store {
    pub(in crate::runtime) fn activate_scheduled(
        &self,
        row: &Scheduled,
        state: MessageState,
    ) -> Result<i64> {
        let sequence = self.move_scheduled(row, state, false)?;
        self.execute_cached(queries::PROMOTE_SCHEDULED, params![state, row.id])?;
        Ok(sequence)
    }
    pub(in crate::runtime) fn record_promotion(
        &self,
        agent: &crate::runtime::domain::Agent,
        row: &Scheduled,
        sequence: i64,
    ) -> Result<()> {
        self.event(
            agent,
            EventKind::MessagePromoted,
            api::value(events::MessagePromoted {
                id: &row.id,
                not_before: row.at,
                seq: sequence,
            }),
        )?;
        self.changed(&row.recipient);
        Ok(())
    }

    pub(crate) fn enqueue_scheduled(
        &self,
        id: &str,
        sender: &str,
        recipient: &str,
        body: &str,
        at: u64,
    ) -> Result<()> {
        let at = i64::try_from(at).map_err(|cause| {
            Error::invalid_request("Invalid message deadline.").with_source(cause)
        })?;
        self.execute_cached(
            queries::ENQUEUE_SCHEDULED,
            params![id, sender, recipient, body, at],
        )?;
        self.execute_cached(queries::ENQUEUE_SCHEDULED_2, params![id, sender])?;
        self.deadlines_changed();
        Ok(())
    }

    pub(in crate::runtime) fn quarantine_scheduled(
        &self,
        row: &Scheduled,
        uncertain: bool,
    ) -> Result<()> {
        self.move_scheduled(
            row,
            if uncertain {
                MessageState::Uncertain
            } else {
                MessageState::Failed
            },
            true,
        )?;
        self.event(
            &self.agent(&row.recipient)?,
            EventKind::MessageScheduleFailed,
            api::value(events::MessageScheduleFailed {
                id: &row.id,
                reason: "invalid_delivery_proof",
            }),
        )?;
        Ok(())
    }

    // The lifecycle owner retains one transaction for cancellation and its cascade.
    pub(super) fn cancel_scheduled(&self, recipient: &str) -> Result<()> {
        loop {
            let rows = self
                .sql()
                .cached()
                .query_rows::<Scheduled>(queries::CANCEL, [recipient])?;
            if rows.is_empty() {
                return Ok(());
            }
            for row in rows {
                let ScheduledProof { valid, uncertain } =
                    self.scheduled_proof(&row.id, &row.sender)?;
                if valid {
                    self.execute_cached(queries::CANCEL_SCHEDULED, [row.id])?;
                } else {
                    self.quarantine_scheduled(&row, uncertain)?;
                }
            }
        }
    }

    fn move_scheduled(
        &self,
        row: &Scheduled,
        state: MessageState,
        quarantine: bool,
    ) -> Result<i64> {
        let attachments = if row.has_attachments {
            self.attachments(&row.id)?
        } else {
            Vec::new()
        };
        self.execute_cached(queries::MOVE_SCHEDULED, params![row.seq, row.id])?;
        // Normal scheduled rows are constrained to these exact non-synthetic flags.
        // Keeping seq on quarantine avoids an UPDATE trigger rewriting delivery proof.
        self.execute_cached(
            queries::MOVE_SCHEDULED_2,
            params![
                quarantine.then_some(row.seq),
                row.id,
                row.sender,
                row.recipient,
                row.body,
                state,
                row.at,
                quarantine.then_some("invalid_delivery_proof"),
                row.join_id
            ],
        )?;
        let sequence = self.connection.last_insert_rowid();
        self.attach(&row.id, &attachments)?;
        Ok(sequence)
    }
}

pub(in crate::runtime) struct ScheduledProof {
    pub valid: bool,
    pub uncertain: bool,
}
impl Queries {
    pub(in crate::runtime) fn due_scheduled(&self, now: i64) -> Result<Vec<Scheduled>> {
        self.sql().cached().query_rows(queries::DUE, [now])
    }

    pub(in crate::runtime) fn scheduled_proof(
        &self,
        id: &str,
        sender: &str,
    ) -> Result<ScheduledProof> {
        // A scheduled input cannot already have any recipient execution or result.
        // Keep the exact proof on failure; never repair/rebind it to current state.
        self.query_row_cached(queries::SCHEDULED_PROOF, params![id, sender], |r| {
            Ok(ScheduledProof {
                valid: r.get(0)?,
                uncertain: r.get(1)?,
            })
        })
    }
}
