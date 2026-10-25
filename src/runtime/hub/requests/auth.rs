//! Authentication request preflight and writer revalidation.
use super::prepared::{AgentSnapshot, Captured, Preparation};
use super::{Caller, Core, Error, Facts, Hub, RequestEnvelope, Result};
use crate::runtime::api::InputContext as Op;
use crate::runtime::{
    api::Request, auth_fallback::prepare, core::eligible, domain::OptionsSelection,
};
use std::sync::Arc;

struct RestorationRevision {
    profile: String,
    generation: uuid::Uuid,
    revision: u64,
}
pub(super) struct AuthPreparation;
pub(super) struct Input {
    input: prepare::Input,
    snapshot: Option<AgentSnapshot<OptionsSelection>>,
}
pub(super) struct Prepared {
    auth: prepare::Prepared,
    snapshot: Option<AgentSnapshot<OptionsSelection>>,
    observed: Option<RestorationRevision>,
}
impl Preparation for AuthPreparation {
    type Input = Input;
    type Prepared = Prepared;
    const BUSY: &'static str = "Login selection changed repeatedly; retry.";

    fn capture(
        core: &mut Core,
        owner: &Caller,
        _: &str,
        request: &RequestEnvelope,
    ) -> Result<Captured<Self::Input>> {
        let (target, snapshot) = match &request.request {
            Request::SetAuth(args) => {
                let agent = core
                    .store()
                    .agent(args.target.required(Op::SetAuth, "/target")?)?;
                eligible(&agent)?;
                let auth = args.selection(core.config(), agent.profile.provider)?;
                (
                    auth.to_owned(),
                    Some(AgentSnapshot::capture(&agent, agent.boundary_selection())),
                )
            }
            Request::LoginRestored(args) if owner.is_human() || owner.is_system() => {
                let auth = args.auth.required(Op::LoginRestored, "/auth")?;
                core.config().profile(auth)?;
                (auth.to_owned(), None)
            }
            _ => {
                return Err(Error::unsupported_operation(
                    "Login restoration is for human callers only.",
                ));
            }
        };
        Ok(Captured::Input(Input {
            input: prepare::Input::capture(core, target),
            snapshot,
        }))
    }
    async fn prepare(
        hub: &Hub,
        request: &RequestEnvelope,
        input: Self::Input,
    ) -> Result<Self::Prepared> {
        let mut auth = hub.preflight.auth(input.input).await?;
        let observed = if matches!(request.request, Request::LoginRestored(_)) {
            hub.restoration_revision(&auth).await?
        } else {
            None
        };
        auth.discover(&hub.catalogs).await;
        Ok(Prepared {
            auth,
            snapshot: input.snapshot,
            observed,
        })
    }
    fn tickets(prepared: &Self::Prepared) -> Vec<Arc<crate::state::RuntimeLease>> {
        prepared.auth.tickets()
    }
    fn still_fresh(core: &Core, prepared: &Self::Prepared) -> Result<bool> {
        if !prepared
            .auth
            .members
            .iter()
            .all(crate::runtime::preflight::request::MemberFacts::fresh)
        {
            return Ok(false);
        }
        if let Some(observed) = &prepared.observed
            && core
                .store()
                .availability(&observed.profile, observed.generation)?
                .revision
                != observed.revision
        {
            return Ok(false);
        }
        if let Some(before) = &prepared.snapshot {
            let current = core.store().agent(&before.id)?;
            if !before.matches(&current, &current.boundary_selection()) {
                return Ok(false);
            }
        }
        Ok(true)
    }
    fn into_facts(prepared: Self::Prepared) -> Result<Facts> {
        let mut facts = Facts::default();
        facts.auth = Some(prepared.auth);
        Ok(facts)
    }
}
impl Hub {
    async fn restoration_revision(
        &self,
        prepared: &prepare::Prepared,
    ) -> Result<Option<RestorationRevision>> {
        let Some(member) = prepared.members.first() else {
            return Ok(None);
        };
        let Ok(binding) = &member.binding else {
            return Ok(None);
        };
        let profile = member.auth.clone();
        let generation = binding.id;
        let observed = self
            .with(move |core| {
                let revision = core.store().availability(&profile, generation)?.revision;
                Ok(RestorationRevision {
                    profile,
                    generation,
                    revision,
                })
            })
            .await?;
        self.catalogs.invalidate(generation);
        Ok(Some(observed))
    }
}
