//! Admission and parking of subscription-bound work.
use super::{AuthReason, probes::retry_seconds};
use super::{Availability, AvailabilityState, WaitingReason};
use crate::{
    error::Result,
    runtime::{
        Core,
        domain::{Agent, MessageState},
        store::Message,
    },
};

impl Core {
    #[expect(
        clippy::expect_used,
        reason = "The matching probe agent and its input share one typed Probe identity."
    )]
    pub(in crate::runtime) fn auth_admit(
        &self,
        agent: &mut Agent,
        message: &Message,
    ) -> Result<bool> {
        if agent.auth_group.is_none()
            && !agent.branch_unbound()
            && agent.profile.method != crate::config::Method::ApiKey
        {
            return Ok(true);
        }
        let now = crate::state::now();
        if self.return_to_plan(agent)? {
            return Ok(false);
        }
        let mut value = self
            .store()
            .availability(&agent.auth, agent.auth_generation)?;
        // Already loaded conversations keep their captured credential. They may
        // settle input, but cannot borrow another key's probe or confirmation.
        if agent.execution_key != value.key {
            return Ok(true);
        }
        // The deadline owner, rather than admission, converts an unknown deadline
        // into permission. This ensures the first retry is always gated.
        let allowed = value.usable(now, Some(&agent.id))
            && (value.state == AvailabilityState::Available || value.retry_at.is_none());
        if !allowed {
            return self.park_auth(agent, &value);
        }
        if value.probe_agent() == Some(&agent.id) {
            if value.probe_id() != agent.auth_probe_id.as_deref() {
                return self.park_auth(agent, &value);
            }
            if value.probe_message() != Some(&message.id) {
                let old = value.probe_message().expect("probe input");
                if self.store().probe_input(&agent.id, old)? != Some(MessageState::Accepted) {
                    return self.park_auth(agent, &value);
                }
                // Startup/redirect priority may change while the provider launches.
                value
                    .probe
                    .as_mut()
                    .expect("owned probe")
                    .message
                    .clone_from(&message.id);
                self.store().save_availability(&value)?;
            }
        } else if value.retry_at.is_some() {
            value.probe = Some(crate::runtime::auth_fallback::Probe {
                id: uuid::Uuid::new_v4().to_string(),
                agent: agent.id.clone(),
                message: message.id.clone(),
                deadline: now + retry_seconds(value.reason),
                error: None,
            });
            self.store().save_availability(&value)?;
        }
        agent.auth_probe_id = value.probe_id().map(str::to_owned);
        self.store().save(agent)?;
        Ok(true)
    }
    fn return_to_plan(&self, agent: &mut Agent) -> Result<bool> {
        if agent.profile.method != crate::config::Method::ApiKey
            || agent.branch_unbound()
            || agent.branch_hold > 0
        {
            return Ok(false);
        }
        let Some(group) = agent.auth_group.clone() else {
            return Ok(false);
        };
        let revision = self.store().auth_revision()?;
        if revision <= agent.auth_checked_revision {
            return Ok(false);
        }
        for profile in self.config().auth_members(&group)? {
            if self.config().profile(&profile)?.method == crate::config::Method::Subscription
                && self
                    .store()
                    .auth_status(&profile)?
                    .availability
                    .iter()
                    .any(|value| {
                        value.confirmed_at.is_some() && value.usable(crate::state::now(), None)
                    })
            {
                self.request_auth_boundary(agent, group, AuthReason::UsageLimit)?;
                return Ok(true);
            }
        }
        agent.auth_checked_revision = revision;
        Ok(false)
    }
    fn park_auth(&self, agent: &mut Agent, value: &Availability) -> Result<bool> {
        if (value.state != AvailabilityState::Available || value.probe_error().is_some())
            && !agent.branch_unbound()
            && agent.branch_hold == 0
            && agent.auth_switch_pending.is_none()
        {
            self.request_auth_boundary(
                agent,
                agent
                    .auth_group
                    .clone()
                    .unwrap_or_else(|| agent.auth.clone()),
                AuthReason::UsageLimit,
            )?;
            return Ok(false);
        }
        agent.waiting_reason = Some(WaitingReason::Quota);
        agent.quota_wait_until = (value.state == AvailabilityState::Limited)
            .then(|| value.deadline())
            .flatten();
        agent.auth_checked_revision = self.store().auth_revision()?;
        self.store().save(agent)?;
        self.store().deadlines_changed();
        Ok(false)
    }
}
