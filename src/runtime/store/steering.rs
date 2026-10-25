//! Durable input bound to one execution and turn. Never participates in admission.
pub(super) mod queries;
use super::{Message, Queries, Store, rows};
use crate::{
    error::{Error, Result},
    runtime::domain::{Agent, MessageState},
};
use rusqlite::params;
use serde_json::Value;

impl Queries {
    pub fn next_steering(&self, id: &str, execution: &str, turn: &str) -> Result<Option<Message>> {
        let mut message = self.sql().cached().query_opt(
            queries::NEXT_STEERING,
            params![id, execution, turn],
            rows::message,
        )?;
        if let Some(m) = &mut message {
            self.hydrate_attachments(m)?;
        }
        Ok(message)
    }
    /// Validate a saved reply without refreshing it from today's delivery state.
    /// The original response, including nested batch acknowledgements, is immutable.
    pub(super) fn validate_replay(response: &Value) -> Result<()> {
        let invalid = || Error::database_error("Invalid saved request response.");
        super::api::DaemonReply::new(response).map_err(|cause| invalid().with_source(cause))?;
        if response.get("results").is_some() {
            let batch: super::api::BatchResponse = crate::runtime::domain::decode::object(response)
                .map_err(|cause| invalid().with_source(cause))?;
            for result in &batch.results {
                Self::validate_replay(result)?;
            }
        }
        if response.get("steering_target").is_none() {
            return Ok(());
        }
        let _: super::api::SendResponse = crate::runtime::domain::decode::object(response)
            .map_err(|cause| invalid().with_source(cause))?;
        Ok(())
    }
}

impl Store {
    pub fn enqueue_steering(
        &self,
        id: &str,
        sender: &str,
        agent: &Agent,
        body: &str,
    ) -> Result<()> {
        self.execute_cached(queries::INSERT_MESSAGE, params![id, sender, agent.id, body])?;
        self.execute_cached(
            queries::INSERT_DELIVERY,
            params![
                id,
                sender,
                agent.turn_id,
                agent.execution_id,
                agent.provider_id(),
                agent.thread_id
            ],
        )?;
        self.changed(&agent.id);
        Ok(())
    }

    /// A shard failure can settle delivery while the worker holds an old snapshot
    /// or a ready ACK. Never overwrite that outcome (especially uncertainty).
    pub fn steering_state(
        &self,
        message: &Message,
        expected: MessageState,
        status: MessageState,
        error: Option<&str>,
    ) -> Result<bool> {
        let changed = self.execute_cached(
            queries::SET_MESSAGE_STATE,
            params![status, message.id, message.sender, expected],
        )?;
        if changed == 0 {
            return Ok(false);
        }
        self.execute_cached(queries::SET_DELIVERY_ERROR, params![error, message.id])?;
        self.changed(&message.recipient);
        Ok(true)
    }

    pub fn expire_steering(&self, id: &str) -> Result<()> {
        self.sql().cached().execute(queries::EXPIRE, [id])?;
        Ok(())
    }

    pub fn finish_steering(
        &self,
        id: &str,
        execution: &str,
        turn: &str,
        status: MessageState,
    ) -> Result<()> {
        self.expire_steering(id)?;
        self.sql()
            .cached()
            .execute(queries::FINISH, params![id, execution, turn, status])?;
        Ok(())
    }
}
