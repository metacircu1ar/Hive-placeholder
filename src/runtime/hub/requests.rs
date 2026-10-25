//! Request preflight is outside every writer transaction; replay and ownership
//! are checked again on return before the facts can authorize any mutation.
use super::{Caller, Error, Hub, Result};
mod auth;
mod key_source;
mod model;
mod prepared;
use crate::runtime::{
    Core,
    api::{Request, RequestEnvelope},
    core::preflight::{self, Facts, RequestInput},
};
use prepared::{Captured, Preparation};
use serde_json::Value;
use std::sync::Arc;

impl Hub {
    pub(in crate::runtime) async fn dispatch(
        &self,
        caller: Caller,
        id: String,
        args: RequestEnvelope,
    ) -> Result<Value> {
        #[cfg(not(feature = "comb"))]
        let acceptance = matches!(
            &args.request,
            Request::Send(_) | Request::Spawn(_) | Request::Batch(_)
        );
        let result = Box::pin(self.dispatch_saved(caller, id, args)).await?;
        #[cfg(not(feature = "comb"))]
        let result = if acceptance {
            let original = result.clone();
            match self
                .with(move |core| {
                    let mut result = result;
                    core.store().decorate_holds(&mut result)?;
                    Ok(result)
                })
                .await
            {
                Ok(value) => value,
                Err(error) => {
                    crate::observability::observation_failed("accepted_reply_holds", &error);
                    original
                }
            }
        } else {
            result
        };
        Ok(result)
    }
    async fn dispatch_saved(
        &self,
        caller: Caller,
        id: String,
        args: RequestEnvelope,
    ) -> Result<Value> {
        // Internal services have their own replay namespace and one explicit operation.
        if caller.is_system() {
            args.request.caller_policy(&caller)?;
        }
        if let Request::KeySource(args) = &args.request {
            return self.key_source(caller, args.clone()).await;
        }
        if matches!(&caller, Caller::Agent { .. })
            && matches!(&args.request, Request::Agents(args) if args.stalled_requested())
        {
            return self
                .reader
                .get()
                .ok_or_else(|| Error::daemon_stopping("Stalled group reader is unavailable."))?
                .stalled(caller, args)
                .await;
        }
        if matches!(
            args.request,
            Request::SetAuth(_) | Request::LoginRestored(_)
        ) {
            return self
                .prepared::<auth::AuthPreparation>(caller, id, args)
                .await;
        }
        if matches!(args.request, Request::SetOptions(_)) {
            return self
                .prepared::<model::ModelPreparation>(caller, id, args)
                .await;
        }
        if !preflight::needed(&args) {
            return self
                .with(move |core| {
                    caller.check(core)?;
                    core.dispatch(&caller, &id, &args, &Facts::default())
                })
                .await;
        }
        if has_attachments(&args) {
            // A cancelled socket/tool caller drops interest, not publication ownership.
            // This owner runs preparation, COMMIT/rollback and orphan cleanup to completion.
            let hub = self.clone();
            return tokio::spawn(async move {
                hub.prepared::<RequestPreparation>(caller, id, args).await
            })
            .await
            .map_err(|e| Error::state_io("Attachment admission did not finish.").with_source(e))?;
        }
        self.prepared::<RequestPreparation>(caller, id, args).await
    }
}

fn has_attachments(args: &RequestEnvelope) -> bool {
    args.request.attachment_paths().is_some_and(|p| {
        !p.is_missing() && !matches!(p, crate::runtime::api::Input::Valid(v) if v.is_empty())
    }) || args.batch_items().is_some_and(|items| {
        items.iter().any(|item| {
            item.operation()
                .is_ok_and(|op| has_attachments(&op.request))
        })
    })
}

struct RequestPreparation;
impl Preparation for RequestPreparation {
    type Input = Box<RequestInput>;
    type Prepared = Facts;
    const BUSY: &'static str =
        "Agent changed repeatedly during request preparation; retry the request.";

    fn capture(
        core: &mut Core,
        caller: &Caller,
        id: &str,
        request: &RequestEnvelope,
    ) -> Result<Captured<Self::Input>> {
        let input = RequestInput::capture(core, caller, request);
        if input.is_empty() {
            return core
                .dispatch(caller, id, request, &Facts::default())
                .map(Captured::Replay);
        }
        Ok(Captured::Input(Box::new(input)))
    }

    async fn prepare(
        hub: &Hub,
        _: &RequestEnvelope,
        mut input: Self::Input,
    ) -> Result<Self::Prepared> {
        if input.has_attachments() {
            input.attachment_guard = Some(Arc::new(hub.attachment_gate.clone().lock_owned().await));
        }
        let mut facts = hub.preflight.request(input).await?;
        facts.discover(&hub.catalogs).await;
        Ok(facts)
    }

    fn attachments(prepared: &Self::Prepared) -> Option<crate::runtime::attachments::Guard> {
        prepared.attachment_guard.clone()
    }

    fn publication(prepared: &Self::Prepared) -> Option<crate::runtime::attachments::Publication> {
        prepared.attachment_publication.clone()
    }
    fn tickets(prepared: &Self::Prepared) -> Vec<Arc<crate::state::RuntimeLease>> {
        prepared.tickets()
    }

    fn still_fresh(core: &Core, prepared: &Self::Prepared) -> Result<bool> {
        prepared.fresh(core)
    }

    fn into_facts(prepared: Self::Prepared) -> Result<Facts> {
        Ok(prepared)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
