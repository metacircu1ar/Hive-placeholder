//! Model catalog discovery keeps stale selections from authorizing a writer change.
use super::prepared::{AgentSnapshot, Captured, Preparation};
use super::{Caller, Core, Facts, Hub, RequestEnvelope, Result};
use crate::runtime::api::InputContext as Op;
use crate::{
    config::Profile,
    runtime::{
        api::Request,
        core::eligible,
        provider::{catalogs::BindingInput, protocol::Model},
    },
};

pub(super) struct ModelPreparation;
pub(super) struct Prepared {
    snapshot: AgentSnapshot<Profile>,
    catalog: Result<Vec<Model>>,
    credential: Option<crate::auth::Credential>,
}

impl Preparation for ModelPreparation {
    type Input = Box<(AgentSnapshot<Profile>, BindingInput)>;
    type Prepared = Prepared;
    const BUSY: &'static str = "Agent selection changed repeatedly; retry the request.";

    fn capture(
        core: &mut Core,
        _: &Caller,
        _: &str,
        request: &RequestEnvelope,
    ) -> Result<Captured<Self::Input>> {
        let Request::SetOptions(change) = &request.request else {
            unreachable!()
        };
        let agent = core
            .store()
            .agent(change.target.required(Op::SetOptions, "/target")?)?;
        eligible(&agent)?;
        change.selection(&agent)?;
        let binary = core.binaries().resolve(agent.profile.provider)?;
        let snapshot = AgentSnapshot::capture(&agent, agent.profile.clone());
        Ok(Captured::Input(Box::new((
            snapshot,
            BindingInput {
                credential: None,
                keys: core.leases().keys.clone(),
                auth: agent.auth,
                profile: agent.profile,
                generation: agent.auth_generation,
                binary,
                daemon_lock: core.daemon_lock().clone(),
                launcher: core.launcher().clone(),
                state: core.state().clone(),
            },
        ))))
    }
    async fn prepare(hub: &Hub, _: &RequestEnvelope, input: Self::Input) -> Result<Self::Prepared> {
        let (snapshot, input) = *input;
        let (catalog, credential) = match hub.catalogs.lookup_binding(input).await {
            Ok((models, credential)) => (Ok(models), Some(credential)),
            Err(error) => (Err(error), None),
        };
        Ok(Prepared {
            snapshot,
            catalog,
            credential,
        })
    }
    fn still_fresh(core: &Core, prepared: &Self::Prepared) -> Result<bool> {
        if prepared
            .credential
            .as_ref()
            .is_some_and(|credential| !credential.fresh())
        {
            return Ok(false);
        }
        let current = core.store().agent(&prepared.snapshot.id)?;
        eligible(&current)?;
        Ok(prepared.snapshot.matches(&current, &current.profile))
    }
    fn into_facts(prepared: Self::Prepared) -> Result<Facts> {
        let mut facts = Facts::default();
        facts.model_catalog = Some(prepared.catalog?);
        facts.model_key = prepared
            .credential
            .as_ref()
            .and_then(|value| value.authority().cloned());
        Ok(facts)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Controlled in-memory request and private state fixtures.
mod tests {
    use super::*;
    use crate::runtime::test_support::{AgentBuilder, CoreBuilder};

    #[test]
    fn malformed_selection_is_rejected_during_capture_without_catalog_work() {
        let temp = crate::runtime::test_directory();
        let mut core = CoreBuilder::new(temp.path()).build();
        let agent = AgentBuilder::new("agent").build();
        core.store().save(&agent).unwrap();
        for (model, effort, code) in [
            ("bad name", "high", "invalid_value"),
            ("sonnet", "fast", "invalid_value"),
        ] {
            let request = RequestEnvelope::new(
                serde_json::json!({"op":"set_options", "target":"agent","options":{"model":model,"effort":effort}}),
            );
            let Err(error) =
                ModelPreparation::capture(&mut core, &Caller::Human, "request", &request)
            else {
                panic!("invalid selection reached catalog preparation")
            };
            assert_eq!(error.code(), code);
            assert_eq!(core.store().agent("agent").unwrap().model, agent.model);
        }
    }
}
