//! Atomic fan-out with optional durable collection of each consuming turn's reply.
use super::{Caller, Core, Facts, Request, RequestEnvelope, api, invalid_request};
use crate::{
    error::{Error, Result},
    runtime::{
        api::{Input, batch, joins::JoinAccepted},
        domain::DeliveryMode,
    },
};
use serde_json::Value;
use std::collections::HashSet;

impl Core {
    pub(super) fn batch(
        &mut self,
        caller: &Caller,
        request_id: &str,
        args: &api::BatchArgs,
        envelope: &RequestEnvelope,
        facts: &Facts,
    ) -> Result<Value> {
        let items = envelope.checked_batch_items()?;
        let join_options = args.join_options()?;
        if join_options.is_some()
            && !caller.is_human()
            && self
                .store
                .agent(caller.name())?
                .completion_pending
                .is_some()
        {
            return Err(Error::completion_pending(
                "Completion is pending the turn boundary; a join started now would be cancelled.",
            ));
        }
        let join = join_options
            .map(|options| {
                self.store.create_join(
                    caller.name(),
                    request_id,
                    options.deadline(crate::state::now())?,
                )
            })
            .transpose()?;
        let mut results = Vec::new();
        let mut members = Vec::new();
        let mut ids = HashSet::new();
        for (index, item) in items.iter().enumerate() {
            let api::BatchOperation { id, request } = item
                .operation()
                .map_err(|error| batch::at_index(error, index))?;
            if join.is_some() {
                self.validate_join_member(caller, &id, request_id, &request.request, &mut ids)
                    .map_err(|error| batch::at_index(error, index))?;
            }
            let response = self
                .dispatch(caller, &id, &request, facts)
                .map_err(|error| batch::at_index(error, index))?;
            if let Some(join) = &join {
                let message = joined_message(&request.request, &response)?;
                if let Some(message) = message {
                    self.store
                        .join_member(join, caller.name(), &message, members.len())?;
                    members.push(message);
                }
            }
            results.push(response);
        }
        // An inner request may have become replayable after preflight. Its failed
        // capture must not allow other, deliberately unpublished snapshots to commit.
        facts.attachments_published()?;
        if let Some(join) = &join {
            if members.is_empty() {
                return Err(batch::error(
                    hive_input::Code::ConflictingArguments,
                    "/operations",
                    "a joined batch with at least one spawn or send operation",
                ));
            }
            self.store
                .join_requested(join, caller.name(), request_id, members.len())?;
        }
        Ok(api::value(api::BatchResponse {
            results,
            join: join.map(|join_id| JoinAccepted { join_id, members }),
        }))
    }

    fn validate_join_member(
        &self,
        caller: &Caller,
        id: &str,
        outer: &str,
        request: &Request,
        ids: &mut HashSet<String>,
    ) -> Result<()> {
        #[cfg(not(feature = "comb"))]
        if let Request::Send(send) = request {
            crate::runtime::features::require(&self.store.relative_target(
                caller,
                send.target.required(api::InputContext::Send, "/target")?,
                api::InputContext::Send,
            )?)?;
        }
        if id == outer
            || !ids.insert(id.to_owned())
            || self.store.request_used(caller.name(), id)?
        {
            return Err(Error::request_conflict(
                "A new joined batch requires fresh, distinct operation request IDs.",
            ));
        }
        if let Request::Send(send) = request
            && (!matches!(
                send.mode,
                Input::Missing | Input::Valid(DeliveryMode::NextTurn)
            ) || send.target.as_str() == Some(caller.name()))
        {
            return Err(batch::error(
                hive_input::Code::ConflictingArguments,
                if send.target.as_str() == Some(caller.name()) {
                    "/target"
                } else {
                    "/mode"
                },
                "join sends require next-turn delivery to another agent",
            ));
        }
        Ok(())
    }
}

fn joined_message(request: &Request, response: &Value) -> Result<Option<String>> {
    let message = if matches!(request, Request::Send(_) | Request::Spawn(_)) {
        Some(
            response
                .as_object()
                .and_then(|fields| fields.get("message_id"))
                .and_then(Value::as_str)
                .ok_or_else(invalid_request)?
                .to_owned(),
        )
    } else {
        None
    };
    Ok(message)
}
