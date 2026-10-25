//! Explicit login selection and restored-login updates.
use super::AuthReason;
use super::AvailabilityState;
use crate::runtime::api::InputContext as Op;
use crate::runtime::api::{self, events};
use crate::{
    error::Result,
    runtime::{
        Core,
        domain::{EventKind, ProviderEventKind},
    },
};

impl Core {
    pub(in crate::runtime) fn set_auth(
        &mut self,
        args: &crate::runtime::api::SetAuthArgs,
        facts: &crate::runtime::core::preflight::Facts,
    ) -> Result<serde_json::Value> {
        let mut agent = self
            .store()
            .agent(args.target.required(Op::SetAuth, "/target")?)?;
        crate::runtime::core::eligible(&agent)?;
        check_switch_boundary(&agent)?;
        let target = args.selection(self.config(), agent.profile.provider)?;
        let prepared = facts
            .auth
            .as_ref()
            .ok_or_else(|| crate::error::Error::internal_error("Login preflight missing."))?;
        let selected = select_auth(self, &agent, target, prepared)?;
        let attaching = self
            .config()
            .auth_groups
            .get(target)
            .is_some_and(|group| group.contains(&agent.auth))
            && prepared.members[selected.index].auth == agent.auth;
        let member = &prepared.members[selected.index];
        let mut candidate = agent.clone();
        candidate.auth.clone_from(&member.auth);
        if !self
            .store()
            .admits_process_options(&candidate, self.config().runtime.max_process_variants)?
        {
            return Err(Op::SetAuth.error(
                hive_input::Code::InvalidValue,
                "/auth",
                "a login with capacity within runtime.max_process_variants",
            ));
        }
        if (attaching
            || (agent.waiting_reason.is_none()
                && agent.state != crate::runtime::domain::AgentState::Failed))
            && !selected.waiting()
            && member.auth == agent.auth
            && member
                .binding
                .as_ref()
                .is_ok_and(|b| b.id == agent.auth_generation)
        {
            self.attach_current_auth(&mut agent, target)?;
        } else {
            self.request_auth_boundary(&mut agent, target.into(), AuthReason::Manual)?;
        }
        self.store().event(
            &agent,
            EventKind::AuthSwitchRequested,
            api::value(events::AuthSwitchRequested {
                auth: target,
                takes_effect: "next_turn_boundary",
            }),
        )?;
        Ok(api::value(api::AuthChangedResponse {
            agent_id: &agent.id,
            auth: target,
            auth_group: self
                .config()
                .auth_groups
                .contains_key(target)
                .then_some(target),
            availability: self.store().availability(
                &member.auth,
                member.binding.as_ref().map_err(Clone::clone)?.id,
            )?,
            takes_effect: "next_turn_boundary",
        }))
    }

    fn attach_current_auth(
        &mut self,
        agent: &mut crate::runtime::domain::Agent,
        target: &str,
    ) -> Result<()> {
        let group = self.config().auth_groups.get(target);
        agent.auth_group = group.is_some().then(|| target.to_owned());
        agent.auth_decision =
            group.map(|group| super::AuthDecision::new(group, super::AuthChoice::Sticky));
        agent.clear_auth_wait(self.store().auth_revision()?);
        self.store().save(agent)?;
        self.store().deadlines_changed();
        Ok(())
    }

    pub(in crate::runtime) fn login_restored(
        &mut self,
        args: &crate::runtime::api::AuthArgs,
        facts: &crate::runtime::core::preflight::Facts,
    ) -> Result<serde_json::Value> {
        let auth = args.auth.required(Op::LoginRestored, "/auth")?;
        let prepared = facts
            .auth
            .as_ref()
            .ok_or_else(|| crate::error::Error::internal_error("Login preflight missing."))?;
        let member = prepared
            .members
            .first()
            .ok_or_else(|| crate::error::Error::unknown_profile("Missing login profile."))?;
        let binding = member.binding.clone()?;
        member
            .catalog
            .as_ref()
            .ok_or_else(|| crate::error::Error::internal_error("Login catalog preflight missing."))?
            .as_ref()
            .map_err(Clone::clone)?;
        self.store()
            .select_key_authority(auth, binding.id, member.authority())?;
        if binding.profile.method == crate::config::Method::ApiKey {
            self.restore_api_attempt(auth, binding.id, member.authority())?;
        } else {
            self.restore_verified_auth(auth, binding.id)?;
        }
        self.store().auth_status(auth).map(api::value)
    }

    pub(super) fn restore_api_attempt(
        &self,
        auth: &str,
        generation: uuid::Uuid,
        key: Option<&crate::state::KeyAuthority>,
    ) -> Result<()> {
        let mut value = self.store().availability(auth, generation)?;
        if value.key.as_ref() != key || key.is_none() || value.probe_id().is_some() {
            return Ok(());
        }
        // Explicit login grants a fresh unverified attempt; account/read is not
        // remote validation, and source status never reaches this mutation.
        value.mark_available();
        value.confirmed_at = None;
        value.updated_at = crate::state::now();
        self.store().save_revised(&mut value)?;
        self.store().provider_event(
            &format!("auth:{auth}"),
            ProviderEventKind::LoginRestored,
            api::value(events::LoginRestored {
                profile: auth,
                generation,
                method: "api_key",
            }),
        )
    }

    // Identity verification cannot prove remaining quota or spending permission.
    pub(in crate::runtime) fn restore_verified_auth(
        &self,
        auth: &str,
        generation: uuid::Uuid,
    ) -> Result<()> {
        // A newly verified generation must wake agents still held on its predecessor.
        // Repeated verification of an existing healthy row remains a no-op.
        self.store().ensure_availability(auth, generation)?;
        let mut availability = self.store().availability(auth, generation)?;
        if availability.probe_error().is_some() {
            return Ok(());
        }
        match availability.reason {
            Some(AuthReason::Authentication) => {
                availability.mark_available();
            }
            Some(AuthReason::UsageAndAuthentication)
                if availability.state == AvailabilityState::Unavailable =>
            {
                availability.state = AvailabilityState::Limited;
                // The original quota floor/opportunity remains intact.
                availability.retry_at = Some(availability.retry_at.ok_or_else(|| {
                    crate::error::Error::database_error(
                        "Mixed login hold is missing its retry deadline.",
                    )
                })?);
            }
            _ => return Ok(()),
        }

        availability.updated_at = crate::state::now();
        availability.confirmed_at = None;
        self.store().save_revised(&mut availability)?;
        self.store().provider_event(
            &format!("auth:{auth}"),
            ProviderEventKind::LoginRestored,
            api::value(events::LoginRestored {
                profile: auth,
                generation,
                method: "subscription",
            }),
        )
    }
}

fn check_switch_boundary(agent: &crate::runtime::domain::Agent) -> Result<()> {
    if agent.branch_hold > 0 || agent.branch_unbound() {
        return Err(crate::error::Error::agent_busy(
            "Login changes must wait until native branch materialization finishes.",
        ));
    }
    Ok(())
}

fn select_auth(
    core: &Core,
    agent: &crate::runtime::domain::Agent,
    target: &str,
    prepared: &super::prepare::Prepared,
) -> Result<super::selection::Selection> {
    let boundary = agent.boundary_selection();
    let selected = super::selection::select(super::selection::SelectionRequest {
        queries: core.store(),
        allow_api: agent.api_fallback || !core.config().auth_groups.contains_key(target),
        members: &prepared.members,
        model: &boundary.model,
        effort: &boundary.effort,
        // Explicit profiles override availability; compatible group attachment is sticky.
        follow_availability: false,
        sticky: Some(&agent.auth),
        agent: Some(&agent.id),
        group: core.config().auth_groups.get(target),
        capacity: None,
    })?;
    Ok(selected)
}
