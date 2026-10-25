//! Verified conversation rebinding after a subscription boundary.
use super::AuthReason;
use super::{AuthChoice, AuthDecision, SwitchIntent, WaitingReason, prepare, selection};
use crate::config::Method;
use crate::runtime::api::{self, events};
use crate::{
    error::{Error, Result},
    runtime::{
        Core, Shared,
        domain::{Agent, AgentState, EventKind},
        provider::Pool,
    },
};
#[expect(
    clippy::expect_used,
    reason = "The preceding pending-intent check excludes agents without a switch target."
)]
pub(super) async fn finish_switch(shared: &Shared, pool: &Pool, id: &str) -> Result<()> {
    let key = id.to_owned();
    let captured = shared
        .with(move |core| {
            let agent = core.store().agent(&key)?;
            if core.running().contains(&key)
                || agent.terminal()
                || agent.auth_switch_pending.is_none()
            {
                return Ok(None);
            }
            if agent.recovery_fence.is_some() {
                return Err(Error::recovery_required(
                    "Old execution cleanup must be confirmed before changing login.",
                ));
            }
            let input = prepare::Input::capture(
                core,
                agent
                    .auth_switch_pending
                    .as_ref()
                    .expect("pending")
                    .target
                    .clone(),
            )
            .after_cleanup(agent.clone());
            Ok(Some((agent, input)))
        })
        .await?;
    let Some((before, input)) = captured else {
        return Ok(());
    };
    let mut prepared = shared.preflight.auth(input).await?;
    prepared.discover(&shared.catalogs).await;
    let tickets = prepared.tickets();
    let result = shared
        .with_retained(tickets, move |core| apply_switch(core, &before, &prepared))
        .await;
    if result
        .as_ref()
        .is_err_and(crate::runtime::provider::catalogs::capacity_exhausted)
    {
        pool.drain_catalog();
    }
    result
}

pub(super) fn apply_switch(
    core: &mut Core,
    before: &Agent,
    prepared: &prepare::Prepared,
) -> Result<()> {
    let mut agent = core.store().agent(&before.id)?;
    check_switch(core, &agent, before)?;
    if !prepared
        .members
        .iter()
        .all(crate::runtime::preflight::request::MemberFacts::fresh)
    {
        return Err(Error::agent_busy(
            "API key changed during login switch; retry.",
        ));
    }
    core.auth_uncertain_cutoff(&mut agent)?;
    core.release_auth_probe(&mut agent, None)?;
    let intent = agent
        .auth_switch_pending
        .clone()
        .ok_or_else(|| Error::agent_busy("Login switch no longer pending."))?;
    if keeps_idle_binding(core, &mut agent, &intent)? {
        return Ok(());
    }
    let selected = destination(core, &agent, &intent, prepared)?;
    let group = core.config().auth_groups.get(&intent.target);
    agent.auth_checked_revision = core.store().auth_revision()?;
    if selected.waiting() {
        // A switch into another group records its decision once it completes.
        if let Some(group) = group
            && agent.auth_group.as_deref() == Some(intent.target.as_str())
        {
            agent.auth_decision = Some(AuthDecision::new(group, AuthChoice::Waiting));
        }
        agent.waiting_reason = Some(WaitingReason::Quota);
        agent.quota_wait_until = selected.deadline();
        core.store().save(&agent)?;
        core.store().deadlines_changed();
        return Ok(());
    }
    let member = &prepared.members[selected.index];
    let binding = member.binding.as_ref().map_err(Clone::clone)?;
    if binding.profile.provider != agent.profile.provider {
        return Err(Error::config_invalid("Login provider cannot change."));
    }
    let from = agent.auth.clone();
    let from_method = agent.profile.method;
    check_destination_capacity(core, &agent, &member.auth)?;
    core.store()
        .select_key_authority(&member.auth, binding.id, member.authority())?;
    let reason = decision_reason(&agent, member, intent.reason, selected.choice);
    let changed = agent.auth != member.auth || agent.auth_generation != binding.id;
    if changed {
        rebind(core, &mut agent, member, intent.reason)?;
    }
    agent.auth_decision = group.map(|group| AuthDecision::new(group, reason));
    agent.auth_group = group.is_some().then_some(intent.target);
    agent.clear_auth_wait(agent.auth_checked_revision);
    agent.state = AgentState::Stopped;
    agent.error = None;
    core.store().save(&agent)?;
    publish_switch(
        core,
        &agent,
        selected,
        (&from, from_method),
        intent.reason,
        changed,
    )?;
    Ok(())
}

/// Input may have been cancelled while preflight ran. An idle balanced agent
/// keeps its subscription binding; its next input requests a boundary again.
/// An API-key agent has no plan context to keep, so it still returns to a
/// confirmed plan here, and the keep path leaves `return_to_plan` armed.
pub(super) fn keeps_idle_binding(
    core: &Core,
    agent: &mut Agent,
    intent: &SwitchIntent,
) -> Result<bool> {
    if intent.reason == AuthReason::Manual
        || agent.profile.method != Method::Subscription
        || !core
            .config()
            .auth_groups
            .get(&intent.target)
            .is_some_and(crate::config::AuthGroup::balanced)
        || core.store().deliverable_input(&agent.id)?
    {
        return Ok(false);
    }
    agent.clear_auth_wait(agent.auth_checked_revision);
    if let Some(decision) = &mut agent.auth_decision
        && decision.reason == AuthChoice::Waiting
    {
        decision.reason = AuthChoice::Sticky;
    }
    core.store().save(agent)?;
    Ok(true)
}

fn destination(
    core: &Core,
    agent: &Agent,
    intent: &SwitchIntent,
    prepared: &prepare::Prepared,
) -> Result<selection::Selection> {
    let automatic = intent.reason != AuthReason::Manual;
    let group = core.config().auth_groups.get(&intent.target);
    let boundary = agent.boundary_selection();
    let desired = agent.agent_options.desired_ref();
    selection::select(selection::SelectionRequest {
        queries: core.store(),
        allow_api: agent.api_fallback || group.is_none(),
        members: &prepared.members,
        model: &boundary.model,
        effort: &boundary.effort,
        follow_availability: automatic || group.is_some(),
        sticky: Some(&agent.auth),
        agent: Some(&agent.id),
        group,
        capacity: automatic.then_some(selection::Capacity {
            provider: agent.profile.provider,
            desired: &desired,
            reserved: agent
                .reserved_turn
                .as_ref()
                .map(|reserved| reserved.selection.options.as_ref()),
            limit: core.config().runtime.max_process_variants,
        }),
    })
}

/// Evidence for a completed switch, recorded before the agent is rebound.
fn decision_reason(
    agent: &Agent,
    member: &crate::runtime::core::preflight::MemberFacts,
    reason: AuthReason,
    choice: AuthChoice,
) -> AuthChoice {
    if agent.auth == member.auth {
        AuthChoice::Sticky
    } else if member.authority().is_some() {
        AuthChoice::PaidTail
    } else if reason != AuthReason::Manual && agent.profile.method == Method::Subscription {
        AuthChoice::LimitedMove
    } else {
        choice
    }
}

fn check_switch(core: &Core, agent: &Agent, before: &Agent) -> Result<()> {
    if agent.terminal()
        || agent.branch_hold > 0
        || agent.branch_unbound()
        || core.running().contains(&agent.id)
        || agent.auth_switch_pending != before.auth_switch_pending
        || agent.auth_generation != before.auth_generation
        || agent.execution_id != before.execution_id
        || agent.boundary_selection() != before.boundary_selection()
        || agent.options_revision != before.options_revision
    {
        return Err(Error::agent_busy(
            "Agent changed while preparing login switch; retry.",
        ));
    }
    if agent.recovery_fence.is_some() {
        return Err(Error::recovery_required(
            "Old execution cleanup is unresolved.",
        ));
    }
    Ok(())
}

fn rebind(
    core: &Core,
    agent: &mut Agent,
    member: &crate::runtime::core::preflight::MemberFacts,
    reason: AuthReason,
) -> Result<()> {
    let binding = member.binding.as_ref().map_err(Clone::clone)?;
    if agent.native_input_attempted || agent.handoff.is_some() {
        agent.handoff = Some(core.store().auth_handoff(&agent.id, reason)?);
    }
    if let Some(branch) = core.store().branch_intent(&agent.id)? {
        agent.handoff = Some(format!(
            concat!(
                "{}\n\nBranched from agent {} through native turn {}. ",
                "Switching login discards inherited native context; ",
                "the following is only a bounded summary.\n{}",
            ),
            agent.handoff.as_deref().unwrap_or_default(),
            branch.source,
            branch.source_turn,
            branch.source_handoff.as_deref().unwrap_or_default()
        ));
        core.store().event(
            agent,
            crate::runtime::domain::EventKind::BranchContextDropped,
            api::value(events::BranchContextDropped {
                source: &branch.source,
                source_turn: &branch.source_turn,
                reason,
                mode: "summary_handoff",
            }),
        )?;
    }
    agent.auth.clone_from(&member.auth);
    agent.reset_conversation(binding);
    if !core.store().reservation_accepted(agent)? {
        agent.reserved_turn = None;
    }
    core.store()
        .ensure_availability(&agent.auth, agent.auth_generation)?;
    Ok(())
}

fn publish_switch(
    core: &Core,
    agent: &Agent,
    selected: selection::Selection,
    from: (&str, crate::config::Method),
    reason: AuthReason,
    changed: bool,
) -> Result<()> {
    core.store().skipped_members(agent, &selected.skipped)?;
    if changed {
        core.store().event(
            agent,
            EventKind::AuthSwitched,
            api::value(events::AuthSwitched {
                from: from.0,
                to: &agent.auth,
                from_method: from.1.as_str(),
                to_method: agent.profile.method.as_str(),
                reason,
                mode: "handoff",
                auth_group: agent.auth_group.as_deref(),
                auth_decision: agent.auth_decision,
            }),
        )?;
    }
    Ok(())
}

fn check_destination_capacity(core: &Core, agent: &Agent, auth: &str) -> Result<()> {
    let mut candidate = agent.clone();
    auth.clone_into(&mut candidate.auth);
    if !core
        .store()
        .admits_process_options(&candidate, core.config().runtime.max_process_variants)?
    {
        return Err(Error::process_variant_limit(
            "Login destination has no capacity for these process settings; resume after \
                capacity changes.",
        ));
    }
    Ok(())
}
