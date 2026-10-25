//! Agent creation and instruction snapshot preparation.
use super::{
    Agent, Caller, Core, Error, Facts, INSTRUCTION_FILE_BYTES, INSTRUCTION_FILE_READ_BYTES,
    RequestEnvelope, Result, SpawnArgs, SpawnContext, invalid_request, preflight,
};
use crate::runtime::api::InputContext as Op;
use crate::runtime::api::events;
use crate::runtime::auth_fallback::{Selection, SelectionRequest, select};
use crate::{
    config::Provider,
    runtime::{
        api,
        domain::{AgentState, EventKind, TaskOutcome},
    },
    state,
};
use serde_json::Value;
use std::{fs, path::Path};
use uuid::Uuid;

pub(crate) fn text_file(path: &Path) -> Result<String> {
    let file = fs::File::open(path).map_err(|cause| {
        Error::instructions_unavailable("Cannot read a configured instruction file.")
            .with_source(cause)
    })?;
    let text = crate::os::read_text_capped(file, INSTRUCTION_FILE_READ_BYTES)
        .map_err(|cause| invalid_request().with_source(cause))?;
    if text.len() > INSTRUCTION_FILE_BYTES {
        return Err(invalid_request());
    }
    Ok(text)
}

impl Core {
    pub(super) fn spawn(
        &mut self,
        caller: &Caller,
        args: &SpawnArgs,
        envelope: &RequestEnvelope,
        facts: &Facts,
    ) -> Result<Value> {
        let choice = self.spawn_choice(args, envelope, facts)?;
        let identity = self.spawn_identity(caller, &choice)?;
        let plan = self.build_spawn_plan(args, choice, identity)?;
        self.persist_spawn(caller, args, plan, facts.attachments(envelope)?)
    }
    fn spawn_choice<'a>(
        &self,
        args: &SpawnArgs,
        envelope: &RequestEnvelope,
        facts: &'a Facts,
    ) -> Result<SpawnChoice<'a>> {
        if self.store.active_count()? >= self.config.runtime.max_agents {
            return Err(Error::agent_limit(
                "Configured nonterminal or draining agent capacity is exhausted.",
            ));
        }
        let proof = facts.spawn(envelope)?;
        let resolved = proof.resolved.clone()?;
        if let Some((id, selection)) = &proof.parent_options {
            let current = self.store.agent(id)?;
            if &crate::runtime::branching::effective_selection(&current) != selection {
                return Err(Error::agent_busy(
                    "Parent options changed during spawn preflight; retry the request.",
                ));
            }
        }
        let source = proof
            .source
            .as_ref()
            .map(|s| s.as_ref().map_err(Clone::clone))
            .transpose()?;
        let intent = source
            .map(|source| {
                let current = self.store.agent(&source.agent.id)?;
                super::super::features::require(&current)?;
                if !source.fresh(&current) {
                    return Err(Error::agent_busy(
                        "Source configuration changed during branch preflight.",
                    ));
                }
                self.store.branch_boundary(&current)
            })
            .transpose()?;
        let context = proof
            .context
            .as_ref()
            .ok_or_else(preflight::missing)?
            .clone()?;
        let selected = self
            .spawn_selection(proof, &resolved, source.is_some())
            .map_err(|error| args.catalog_error(error))?;
        let catalog = proof.members[selected.index]
            .catalog
            .as_ref()
            .ok_or_else(preflight::missing)?
            .as_ref()
            .map_err(Clone::clone)?;
        let model = catalog
            .iter()
            .find(|entry| entry.name() == Some(resolved.model.as_str()))
            .ok_or_else(preflight::missing)?;
        crate::options::catalog(resolved.provider)?
            .validate_capabilities(
                &resolved.options.values,
                model.resolved_name().unwrap_or(&resolved.model),
            )
            .map_err(crate::runtime::api::selection::spawn_options_error)?;
        Ok(SpawnChoice {
            proof,
            resolved,
            source,
            intent,
            context,
            selected,
        })
    }
    /// Branches stay on their source login; other group spawns use its mode.
    fn spawn_selection(
        &self,
        proof: &preflight::SpawnFacts,
        resolved: &crate::config::ResolvedSpawn,
        branch: bool,
    ) -> Result<Selection> {
        let group = self.config.auth_groups.get(&resolved.auth);
        select(SelectionRequest {
            queries: &self.store,
            allow_api: resolved.api_fallback || group.is_none(),
            members: &proof.members,
            model: &resolved.model,
            effort: &resolved.effort,
            follow_availability: branch || group.is_some(),
            sticky: None,
            agent: None,
            group: group.filter(|_| !branch),
            capacity: Some(crate::runtime::auth_fallback::selection::Capacity {
                provider: resolved.provider,
                desired: &resolved.options,
                reserved: None,
                limit: self.config.runtime.max_process_variants,
            }),
        })
    }
    fn spawn_identity(&self, caller: &Caller, choice: &SpawnChoice<'_>) -> Result<SpawnIdentity> {
        let SpawnChoice {
            proof,
            selected,
            resolved,
            ..
        } = choice;
        let member = &proof.members[selected.index];
        let auth = member.auth.clone();
        let binding = member.binding.as_ref().map_err(Clone::clone)?;
        let profile = binding.profile.clone();
        let parent = if caller.is_human() {
            None
        } else {
            Some(self.store.agent(caller.name())?)
        };
        if !self.config.runtime.allow_spawn_widening
            && parent
                .as_ref()
                .is_some_and(|parent| !parent.sandbox.permits_child(resolved.sandbox))
        {
            return Err(Error::binding_conflict(
                "A child cannot have a wider sandbox mode than its spawning agent. Choose \
                    the same or a narrower sandbox.",
            ));
        }
        self.store
            .select_key_authority(&auth, binding.id, member.authority())?;
        let lifetime = Self::check_lifetime(resolved.lifetime, parent.is_some())?;
        Ok(SpawnIdentity {
            auth,
            generation: binding.id,
            profile,
            parent,
            lifetime,
        })
    }
    fn build_spawn_plan(
        &self,
        args: &SpawnArgs,
        choice: SpawnChoice<'_>,
        identity: SpawnIdentity,
    ) -> Result<SpawnPlan> {
        let SpawnChoice {
            proof: _,
            resolved,
            source,
            intent,
            context,
            selected,
        } = choice;
        let id = Uuid::new_v4().to_string();
        // Branches keep the source login unless they wait for it; other group
        // spawns record the chooser's reason.
        let auth_decision = self.config.auth_groups.get(&resolved.auth).map(|group| {
            crate::runtime::auth_fallback::AuthDecision::new(
                group,
                if source.is_some() && !selected.waiting() {
                    crate::runtime::auth_fallback::AuthChoice::Sticky
                } else {
                    selected.choice
                },
            )
        });
        let seed = AgentSeed {
            id: id.clone(),
            // A branch starts unnamed: names label agents, not conversations.
            name: args.name()?.map(str::to_owned),
            role: args
                .role
                .as_str()
                .or(args.preset.as_str())
                .map(str::to_owned)
                .or_else(|| source.and_then(|s| s.agent.role.clone())),
            branched_from_agent_id: source.map(|s| s.agent.id.clone()),
            auth_group: self.config.auth_groups.contains_key(&resolved.auth),
            auth_decision,
            auth_checked_revision: self.store.auth_revision()?,
            resolved,
            context,
            identity,
        };
        Ok(SpawnPlan {
            id,
            agent: seed.build(&selected),
            intent,
            selected,
        })
    }
    fn persist_spawn(
        &self,
        caller: &Caller,
        args: &SpawnArgs,
        plan: SpawnPlan,
        attachments: &[crate::runtime::attachments::Attachment],
    ) -> Result<Value> {
        let SpawnPlan {
            id,
            agent,
            intent,
            selected,
        } = plan;
        if !self
            .store
            .admits_process_options(&agent, self.config.runtime.max_process_variants)?
        {
            return Err(Op::Spawn.error(
                hive_input::Code::InvalidValue,
                "/options",
                "process settings within runtime.max_process_variants for this provider and login",
            ));
        }
        self.store.save(&agent)?;
        if let Some(intent) = &intent {
            self.store.install_branch(&agent, intent)?;
        }
        if selected.waiting() {
            self.store.deadlines_changed();
        }
        self.store.skipped_members(&agent, &selected.skipped)?;
        let message_id = Uuid::new_v4().to_string();
        let body = args.body.required(Op::Spawn, "/body")?;
        let branch_body = intent.as_ref().map(|intent| intent.initial_body(&id, body));
        let body = branch_body.as_deref().unwrap_or(body);
        self.store
            .enqueue_kind(&message_id, caller.name(), &id, body, true)?;
        self.store.attach(&message_id, attachments)?;
        self.store.event(
            &agent,
            EventKind::Spawned,
            api::value(events::Spawned {
                parent_id: agent.parent_id.as_deref(),
                role: agent.role.as_deref(),
                name: agent.name.as_deref(),
                provider: agent.profile.provider,
                auth: &agent.auth,
                auth_group: agent.auth_group.as_deref(),
                auth_decision: agent.auth_decision,
                sandbox: agent.sandbox,
                model: &agent.model,
                effort: &agent.effort,
                lifetime: agent.lifetime,
                attachments,
                initial_message_id: &message_id,
                body,
            }),
        )?;
        Ok(api::value(api::SpawnResponse {
            agent_id: id,
            message_id,
            status: "accepted",
        }))
    }
}

struct SpawnChoice<'a> {
    proof: &'a preflight::SpawnFacts,
    resolved: crate::config::ResolvedSpawn,
    source: Option<&'a crate::runtime::branching::Source>,
    intent: Option<crate::runtime::branching::Intent>,
    context: SpawnContext,
    selected: Selection,
}
struct SpawnPlan {
    id: String,
    agent: Agent,
    intent: Option<crate::runtime::branching::Intent>,
    selected: Selection,
}
struct AgentSeed {
    id: String,
    name: Option<String>,
    role: Option<String>,
    branched_from_agent_id: Option<String>,
    auth_group: bool,
    auth_decision: Option<crate::runtime::auth_fallback::AuthDecision>,
    auth_checked_revision: u64,
    resolved: crate::config::ResolvedSpawn,
    context: SpawnContext,
    identity: SpawnIdentity,
}
impl AgentSeed {
    fn build(self, selected: &Selection) -> Agent {
        let identity = self.identity;
        let resolved = self.resolved;
        let provider = identity.profile.provider;
        Agent {
            tool_catalog: hive_input::compatibility::Exact,
            id: self.id,
            comb: resolved.comb,
            mcp_servers: resolved.mcp_servers,
            role: self.role,
            name: self.name,
            parent_id: identity.parent.map(|p| p.id),
            branched_from_agent_id: self.branched_from_agent_id,
            branch_hold: 0,
            branch_phase: None,
            last_finished_turn: None,
            last_attempted_turn: None,
            auth: identity.auth,
            auth_group: self.auth_group.then_some(resolved.auth),
            auth_decision: self.auth_decision,
            api_fallback: resolved.api_fallback,
            waiting_reason: selected
                .waiting()
                .then_some(crate::runtime::auth_fallback::WaitingReason::Quota),
            quota_wait_until: selected.deadline(),
            auth_switch_pending: None,
            // Native history supplies inherited context. The source's old auth
            // handoff also claims inbox/children/approval ownership; never copy it.
            handoff: None,
            turn_auth_revision: None,
            execution_key: None,
            model_key: None,
            auth_probe_id: None,
            auth_checked_revision: self.auth_checked_revision,
            auth_cutoff: None,
            auth_generation: identity.generation,
            recovery_fence: None,
            conversation_clean: false,
            profile: identity.profile,
            model: resolved.model,
            effort: resolved.effort,
            options_revision: 0,
            applied_options_revision: None,
            agent_options: crate::options::AgentOptions::with_configs(
                resolved.options,
                resolved.native_configs,
            ),
            reserved_turn: None,
            session_selection: None,
            last_accepted_selection: None,
            turn_selection: None,
            redirect_pending: false,
            cwd: self.context.cwd,
            instructions: self.context.instructions,
            provider_process: None,
            lifetime: identity.lifetime,
            sandbox: resolved.sandbox,
            state: AgentState::Starting,
            task_outcome: TaskOutcome::Open,
            paused: false,
            expired: false,
            thread_id: (provider == Provider::Claude).then(|| Uuid::new_v4().to_string()),
            native_input_attempted: false,
            execution_id: None,
            turn_id: None,
            steering: crate::runtime::domain::SteeringCapability::default(),
            control: None,
            completion_pending: None,
            result: None,
            error: None,
            created_at: state::now(),
            ended_at: None,
        }
    }
}

struct SpawnIdentity {
    auth: String,
    generation: Uuid,
    profile: crate::config::Profile,
    parent: Option<Agent>,
    lifetime: crate::config::Lifetime,
}
