//! One capture, prepare and revalidate loop retains evidence through writer commit.
use super::{Caller, Core, Error, Facts, Hub, RequestEnvelope, Result, Value};
use crate::runtime::{core::eligible, domain::Agent};
use std::{future::Future, sync::Arc};

pub(super) enum Captured<T> {
    Replay(Value),
    Input(T),
}

pub(super) trait Preparation: Send + 'static {
    type Input: Send + 'static;
    type Prepared: Send + 'static;
    const BUSY: &'static str;
    fn capture(
        core: &mut Core,
        caller: &Caller,
        id: &str,
        request: &RequestEnvelope,
    ) -> Result<Captured<Self::Input>>;
    fn prepare(
        hub: &Hub,
        request: &RequestEnvelope,
        input: Self::Input,
    ) -> impl Future<Output = Result<Self::Prepared>> + Send;
    fn attachments(_: &Self::Prepared) -> Option<crate::runtime::attachments::Guard> {
        None
    }
    fn publication(_: &Self::Prepared) -> Option<crate::runtime::attachments::Publication> {
        None
    }
    fn tickets(_: &Self::Prepared) -> Vec<Arc<crate::state::RuntimeLease>> {
        Vec::new()
    }
    fn still_fresh(core: &Core, prepared: &Self::Prepared) -> Result<bool>;
    fn into_facts(prepared: Self::Prepared) -> Result<Facts>;
}

#[derive(PartialEq)]
pub(super) struct AgentSnapshot<S> {
    pub id: String,
    auth: String,
    generation: uuid::Uuid,
    revision: u64,
    selection: S,
}
impl<S: PartialEq> AgentSnapshot<S> {
    pub fn capture(agent: &Agent, selection: S) -> Self {
        Self {
            id: agent.id.clone(),
            auth: agent.auth.clone(),
            generation: agent.auth_generation,
            revision: agent.options_revision,
            selection,
        }
    }
    pub fn matches(&self, agent: &Agent, selection: &S) -> bool {
        self.id == agent.id
            && self.auth == agent.auth
            && self.generation == agent.auth_generation
            && self.revision == agent.options_revision
            && &self.selection == selection
    }
}

struct PreparedRequest {
    caller: Caller,
    id: String,
    args: RequestEnvelope,
}
impl PreparedRequest {
    fn capture<P: Preparation>(&self, core: &mut Core) -> Result<Captured<P::Input>> {
        self.caller.check(core)?;
        if let Some(value) = core.replay(&self.caller, &self.id, &self.args)? {
            return Ok(Captured::Replay(value));
        }
        if let Some(agent) = self.caller.agent_id() {
            eligible(&core.store().agent(agent)?)?;
        }
        P::capture(core, &self.caller, &self.id, &self.args)
    }
    fn dispatch<P: Preparation>(
        &self,
        core: &mut Core,
        prepared: P::Prepared,
    ) -> Result<Option<Value>> {
        self.caller.check(core)?;
        if let Some(value) = core.replay(&self.caller, &self.id, &self.args)? {
            return Ok(Some(value));
        }
        if !P::still_fresh(core, &prepared)? {
            return Ok(None);
        }
        let facts = P::into_facts(prepared)?;
        core.dispatch(&self.caller, &self.id, &self.args, &facts)
            .map(Some)
    }
}

impl Hub {
    pub(super) async fn prepared<P: Preparation>(
        &self,
        caller: Caller,
        id: String,
        args: RequestEnvelope,
    ) -> Result<Value> {
        let request = Arc::new(PreparedRequest { caller, id, args });
        // Each writer phase owns the immutable request even if its awaiting task is cancelled.
        // Retry effects remain sequential: capture/replay, external proofs, revalidation/commit.
        for _ in 0..crate::runtime::preflight::MAX_ATTEMPTS {
            let capture = request.clone();
            let captured = self.with(move |core| capture.capture::<P>(core)).await?;
            let input = match captured {
                Captured::Replay(value) => return Ok(value),
                Captured::Input(input) => input,
            };
            let prepared = P::prepare(self, &request.args, input).await?;
            let publication = P::publication(&prepared);
            let tickets = (P::tickets(&prepared), P::attachments(&prepared));
            let dispatch = request.clone();
            let result = self
                .with_retained(tickets, move |core| dispatch.dispatch::<P>(core, prepared))
                .await;
            let result = if let Some(publication) = publication {
                self.settle_publication(publication, result).await
            } else {
                result
            };
            if let Some(value) = result? {
                return Ok(value);
            }
        }
        Err(Error::agent_busy(P::BUSY))
    }
}

#[cfg(test)]
mod tests;
