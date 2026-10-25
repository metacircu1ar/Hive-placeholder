//! Balanced groups: exact weighted choice, capacity, waiting spread and limited moves.
use super::{
    AuthChoice, AuthDecision, AuthReason, Availability, prepare, selection, signals::Signal,
    switch::apply_switch,
};
use crate::{
    config::AuthGroup,
    error::Result,
    runtime::{
        Core,
        domain::{Agent, AgentState},
        provider::protocol::Model,
        test_support::{AgentBuilder, RuntimeFixture},
    },
};
use serde_json::json;

const GROUP: &str = "plans";

/// Bind every member to the fixture profile and return the idle fixture agent.
fn configure(core: &mut Core, group: AuthGroup) -> Result<Agent> {
    let mut agent = core.store().agent("agent")?;
    agent.state = AgentState::Stopped;
    agent.turn_id = None;
    agent.conversation_clean = true;
    agent.auth_group = Some(GROUP.into());
    core.store().save(&agent)?;
    for name in group.profiles() {
        core.config_mut()
            .auth_profiles
            .insert(name.to_owned(), agent.profile.clone());
        if core.state().lookup(name, &agent.profile)?.is_none() {
            core.state().register(name, &agent.profile)?;
        }
    }
    core.config_mut().auth_groups.insert(GROUP.into(), group);
    Ok(agent)
}

fn generation(core: &Core, agent: &Agent, profile: &str) -> uuid::Uuid {
    core.state()
        .lookup(profile, &agent.profile)
        .unwrap()
        .unwrap()
        .id
}

fn prepared(core: &Core, agent: &Agent, previous: Option<&Agent>) -> Result<prepare::Prepared> {
    let input = prepare::Input::capture(core, GROUP.into());
    let input = match previous {
        Some(previous) => input.after_cleanup(previous.clone()),
        None => input,
    };
    let mut prepared = input.resolve()?;
    for member in &mut prepared.members {
        member.catalog = Some(Ok(vec![Model::parse(&json!({
            "model": agent.model,
            "supportedReasoningEfforts": [{"reasoningEffort": agent.effort}],
        }))]));
    }
    Ok(prepared)
}

/// A spawn-like choice: no sticky profile, no agent identity.
fn choose(
    core: &Core,
    agent: &Agent,
    capacity: Option<selection::Capacity<'_>>,
    sticky: Option<&str>,
) -> Result<(String, bool)> {
    let prepared = prepared(core, agent, None)?;
    let selected = selection::select(selection::SelectionRequest {
        queries: core.store(),
        members: &prepared.members,
        model: &agent.model,
        effort: &agent.effort,
        follow_availability: true,
        allow_api: false,
        sticky,
        agent: None,
        group: core.config().auth_groups.get(GROUP),
        capacity,
    })?;
    Ok((
        prepared.members[selected.index].auth.clone(),
        selected.waiting(),
    ))
}

/// A nonterminal bystander bound to `auth`, optionally with an active turn.
fn bind(core: &Core, id: &str, auth: &str, turn: bool) -> Result<Agent> {
    let mut agent = AgentBuilder::new(id).build();
    agent.auth = auth.into();
    agent.state = AgentState::Stopped;
    agent.turn_id = turn.then(|| format!("{id}-turn"));
    core.store().save(&agent)?;
    Ok(agent)
}

fn compression(agent: &Agent, value: bool) -> Result<crate::options::AgentOptions> {
    agent.agent_options.changed(
        agent.profile.provider,
        &[("codex.request_compression".into(), json!(value))].into(),
        None,
    )
}

fn limit(core: &mut Core, agent: &Agent, profile: &str) -> Result<()> {
    let generation = generation(core, agent, profile);
    core.auth_signal(
        profile,
        generation,
        "provider",
        Signal::limited(AuthReason::UsageLimit),
    )
}

/// Request and apply an automatic boundary for `id`, as the deadline owner does.
fn limited_move(core: &mut Core, id: &str) -> Result<Agent> {
    let mut agent = core.store().agent(id)?;
    core.request_auth_boundary(&mut agent, GROUP.into(), AuthReason::UsageLimit)?;
    let prepared = prepared(core, &agent, Some(&agent))?;
    core.atomic(|core| apply_switch(core, &agent, &prepared))?;
    core.store().agent(id)
}

#[tokio::test]
async fn equal_weights_rotate_and_exact_weights_split_four_to_one() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let mut agent = configure(
                core,
                AuthGroup::weighted(&[("main", 1), ("second", 1), ("third", 1)]),
            )?;
            // The fixture agent leaves the group so every member starts empty.
            agent.auth = "outside".into();
            core.store().save(&agent)?;
            let mut order = Vec::new();
            for index in 0..6 {
                let (auth, waiting) = choose(core, &agent, None, None)?;
                assert!(!waiting);
                bind(core, &format!("equal-{index}"), &auth, false)?;
                order.push(auth);
            }
            assert_eq!(
                order,
                ["main", "second", "third", "main", "second", "third"]
            );
            core.config_mut().auth_groups.insert(
                GROUP.into(),
                AuthGroup::weighted(&[("fourth", 4), ("fifth", 1)]),
            );
            for name in ["fourth", "fifth"] {
                core.config_mut()
                    .auth_profiles
                    .insert(name.into(), agent.profile.clone());
                core.state().register(name, &agent.profile)?;
            }
            let mut picks = Vec::new();
            for index in 0..100 {
                let (auth, _) = choose(core, &agent, None, None)?;
                bind(core, &format!("weighted-{index}"), &auth, false)?;
                picks.push(auth);
            }
            // (load + 1) / weight ties at 4/4 against 1/1 keep member order.
            assert_eq!(
                picks[..5],
                ["fourth", "fourth", "fourth", "fourth", "fifth"]
            );
            assert_eq!(picks.iter().filter(|auth| *auth == "fourth").count(), 80);
            assert_eq!(core.store().auth_load("fifth")?.bound_agents, 20);
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}

#[tokio::test]
async fn active_turns_count_ordered_ignores_load_and_sticky_stays() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let mut agent = configure(core, AuthGroup::weighted(&[("main", 1), ("second", 1)]))?;
            agent.auth = "outside".into();
            core.store().save(&agent)?;
            bind(core, "busy", "main", true)?;
            let mut idle = bind(core, "idle", "second", false)?;
            // main: one bound agent plus its active turn outweighs one idle agent.
            assert_eq!(choose(core, &agent, None, None)?.0, "second");
            idle.turn_id = Some("idle-turn".into());
            core.store().save(&idle)?;
            assert_eq!(choose(core, &agent, None, None)?.0, "main");
            // A usable current login is kept even when another member is lighter.
            bind(core, "extra", "main", false)?;
            assert_eq!(choose(core, &agent, None, Some("main"))?.0, "main");
            assert_eq!(choose(core, &agent, None, None)?.0, "second");
            core.config_mut().auth_groups.insert(
                GROUP.into(),
                AuthGroup::ordered(vec!["main".into(), "second".into()]),
            );
            assert_eq!(choose(core, &agent, None, None)?.0, "main");
            assert_eq!(choose(core, &agent, None, Some("second"))?.0, "second");
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}

#[tokio::test]
async fn balanced_choice_skips_variant_full_logins_and_refuses_without_capacity() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let mut agent = configure(
                core,
                AuthGroup::weighted(&[("main", 1), ("second", 1), ("third", 1)]),
            )?;
            agent.auth = "outside".into();
            core.store().save(&agent)?;
            for (id, auth) in [
                ("m1", "main"),
                ("m2", "main"),
                ("m3", "main"),
                ("s2", "second"),
                ("t1", "third"),
                ("t2", "third"),
            ] {
                bind(core, id, auth, false)?;
            }
            let mut other = bind(core, "s1", "second", false)?;
            other.agent_options = compression(&other, true)?;
            core.store().save(&other)?;
            let capacity = |desired| selection::Capacity {
                provider: agent.profile.provider,
                desired,
                reserved: None,
                limit: 1,
            };
            // second ties third as lightest and wins on member order, but already
            // holds two variants; third admits the default one.
            let default = agent.agent_options.desired();
            assert_eq!(choose(core, &agent, None, None)?.0, "second");
            assert_eq!(
                choose(core, &agent, Some(capacity(&default)), None)?.0,
                "third"
            );
            // Ordered groups keep the later destination check instead.
            let ordered = AuthGroup::ordered(vec!["second".into(), "third".into()]);
            let balanced = core
                .config_mut()
                .auth_groups
                .insert(GROUP.into(), ordered)
                .unwrap();
            assert_eq!(
                choose(core, &agent, Some(capacity(&default)), None)?.0,
                "second"
            );
            core.config_mut().auth_groups.insert(GROUP.into(), balanced);
            // No member admits a second variant at a limit of one.
            let compressed = compression(&agent, true)?.desired();
            let error = choose(core, &agent, Some(capacity(&compressed)), None).unwrap_err();
            assert_eq!(error.code(), "process_variant_limit");
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}

#[tokio::test]
async fn waiting_spawns_spread_across_limited_members_that_admit_them() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let mut agent = configure(core, AuthGroup::weighted(&[("main", 1), ("second", 1)]))?;
            agent.auth = "outside".into();
            core.store().save(&agent)?;
            limit(core, &agent, "main")?;
            limit(core, &agent, "second")?;
            bind(core, "first", "main", false)?;
            assert_eq!(choose(core, &agent, None, None)?, ("second".into(), true));
            bind(core, "next", "second", false)?;
            assert_eq!(choose(core, &agent, None, None)?, ("main".into(), true));
            let desired = compression(&agent, true)?.desired();
            let capacity = selection::Capacity {
                provider: agent.profile.provider,
                desired: &desired,
                reserved: None,
                limit: 1,
            };
            let error = choose(core, &agent, Some(capacity), None).unwrap_err();
            assert_eq!(error.code(), "process_variant_limit");
            core.config_mut().auth_groups.insert(
                GROUP.into(),
                AuthGroup::ordered(vec!["main".into(), "second".into()]),
            );
            bind(core, "more", "second", false)?;
            assert_eq!(choose(core, &agent, None, None)?, ("main".into(), true));
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}

#[tokio::test]
async fn limited_login_moves_only_agents_with_input_spread_and_never_back() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let agent = configure(
                core,
                AuthGroup::weighted(&[("main", 1), ("second", 1), ("third", 1)]),
            )?;
            let main = agent.auth_generation;
            for id in ["busy", "idle"] {
                let mut peer = agent.clone();
                peer.id = id.into();
                peer.thread_id = Some(format!("{id}-thread"));
                peer.execution_id = None;
                core.store().save(&peer)?;
            }
            core.store()
                .enqueue("busy-input", "human", "busy", "work")?;
            limit(core, &agent, "main")?;
            let affected = core.store().affected_auth_agents(r#"["plans"]"#)?;
            assert_eq!(affected, ["agent", "busy"]);
            // Ordered groups keep moving every bound agent.
            assert_eq!(
                core.store().affected_auth_agents("[]")?,
                ["agent", "busy", "idle"]
            );
            let moved: Vec<_> = affected
                .iter()
                .map(|id| limited_move(core, id).map(|agent| agent.auth))
                .collect::<Result<_>>()?;
            // Each move counts at once, so a limit spreads rather than piles up.
            assert_eq!(moved, ["second", "third"]);
            let decision = |reason| {
                Some(AuthDecision {
                    mode: crate::config::AuthMode::Balanced,
                    reason,
                })
            };
            assert_eq!(
                core.store().agent("busy")?.auth_decision,
                decision(AuthChoice::LimitedMove)
            );
            let idle = core.store().agent("idle")?;
            assert_eq!((idle.auth.as_str(), idle.auth_generation), ("main", main));
            assert!(idle.waiting_reason.is_none() && idle.auth_switch_pending.is_none());
            // A quota-waiter wake without deliverable input keeps the binding.
            let stayed = limited_move(core, "idle")?;
            assert_eq!(
                (stayed.auth.as_str(), stayed.auth_generation),
                ("main", main)
            );
            assert!(stayed.waiting_reason.is_none() && stayed.auth_switch_pending.is_none());
            assert_eq!(stayed.thread_id, idle.thread_id);
            // Input makes the idle agent movable; it joins the lightest member.
            core.store()
                .enqueue("idle-input", "human", "idle", "work")?;
            assert_eq!(core.store().affected_auth_agents(r#"["plans"]"#)?, ["idle"]);
            assert_eq!(limited_move(core, "idle")?.auth, "second");
            // Recovery moves nobody back; new choices prefer the empty member.
            core.store()
                .save_availability(&Availability::available("main", main))?;
            assert!(
                core.store()
                    .affected_auth_agents(r#"["plans"]"#)?
                    .is_empty()
            );
            assert_eq!(choose(core, &agent, None, None)?.0, "main");
            assert_eq!(core.store().agent("busy")?.auth, "third");
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}

#[tokio::test]
async fn limited_move_skips_variant_full_destination_and_waits_when_all_are_limited() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let mut agent = configure(
                core,
                AuthGroup::weighted(&[("main", 1), ("second", 1), ("third", 1)]),
            )?;
            core.config_mut().runtime.max_process_variants = 1;
            let mut full = bind(core, "full", "second", false)?;
            full.agent_options = compression(&full, true)?;
            core.store().save(&full)?;
            bind(core, "t1", "third", false)?;
            bind(core, "t2", "third", false)?;
            limit(core, &agent, "main")?;
            // second is lighter but full with another variant.
            assert_eq!(limited_move(core, "agent")?.auth, "third");
            // With every member limited the agent waits, still bound.
            agent = core.store().agent("agent")?;
            core.store().enqueue("again", "human", "agent", "work")?;
            for profile in ["second", "third"] {
                limit(core, &agent, profile)?;
            }
            let mut waiting = limited_move(core, "agent")?;
            assert_eq!(waiting.auth, "third");
            assert_eq!(waiting.waiting_reason, Some(super::WaitingReason::Quota));
            let decision = |reason| {
                Some(AuthDecision {
                    mode: crate::config::AuthMode::Balanced,
                    reason,
                })
            };
            assert_eq!(waiting.auth_decision, decision(AuthChoice::Waiting));
            // Resuming on the same recovered login records that it was kept.
            core.store()
                .save_availability(&Availability::available("third", waiting.auth_generation))?;
            assert!(core.unpark_auth_waiter(&mut waiting)?);
            let resumed = core.store().agent("agent")?;
            assert!(resumed.waiting_reason.is_none());
            assert_eq!(resumed.auth_decision, decision(AuthChoice::Sticky));
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}

#[tokio::test]
async fn idle_keep_applies_only_to_subscription_bindings_and_keeps_checks_armed() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let agent = configure(core, AuthGroup::weighted(&[("main", 1), ("second", 1)]))?;
            let mut idle = agent.clone();
            idle.id = "idle".into();
            idle.auth_checked_revision = 0;
            let waiting = AuthDecision {
                mode: crate::config::AuthMode::Balanced,
                reason: AuthChoice::Waiting,
            };
            idle.auth_decision = Some(waiting);
            core.store().save(&idle)?;
            limit(core, &agent, "main")?;
            let intent = |agent: &mut Agent| -> Result<super::SwitchIntent> {
                core.request_auth_boundary(agent, GROUP.into(), AuthReason::UsageLimit)?;
                Ok(agent.auth_switch_pending.clone().unwrap())
            };
            let pending = intent(&mut idle)?;
            assert!(super::switch::keeps_idle_binding(
                core, &mut idle, &pending
            )?);
            let kept = core.store().agent("idle")?;
            assert!(kept.auth_switch_pending.is_none() && kept.waiting_reason.is_none());
            // The keep path does not consume login revisions it did not inspect.
            assert_eq!(kept.auth_checked_revision, 0);
            assert_eq!(
                kept.auth_decision,
                Some(AuthDecision {
                    reason: AuthChoice::Sticky,
                    ..waiting
                })
            );
            // A paid-tail binding still reaches the confirmed-plan restore.
            let mut paid = kept;
            paid.profile.method = crate::config::Method::ApiKey;
            let pending = intent(&mut paid)?;
            assert!(!super::switch::keeps_idle_binding(
                core, &mut paid, &pending
            )?);
            // Manual switches and ordered groups are unchanged.
            paid.profile.method = crate::config::Method::Subscription;
            let manual = super::SwitchIntent {
                reason: AuthReason::Manual,
                ..pending.clone()
            };
            assert!(!super::switch::keeps_idle_binding(
                core, &mut paid, &manual
            )?);
            core.config_mut().auth_groups.insert(
                GROUP.into(),
                AuthGroup::ordered(vec!["main".into(), "second".into()]),
            );
            assert!(!super::switch::keeps_idle_binding(
                core, &mut paid, &pending
            )?);
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}
