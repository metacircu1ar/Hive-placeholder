//! Capture filesystem facts while deferring errors to validation and avoiding replay reads.
pub(in crate::runtime) use super::fs_context::MemberFacts;
use super::fs_context::{FsContext, MemberPurpose};
use crate::error::{Error, Result};
use crate::runtime::api::InputContext as Op;
use crate::runtime::preflight::Identity;
use crate::runtime::provider::catalogs::BindingInput;
use crate::runtime::{
    Core,
    api::{Request, RequestEnvelope, SpawnArgs},
    branching::Source,
    caller::Caller,
    core::SpawnContext,
    domain::Agent,
};
use std::{collections::HashMap, sync::Arc};

pub(in crate::runtime) struct RequestInput {
    attachment_root: std::path::PathBuf,
    pub(in crate::runtime) attachment_guard: Option<crate::runtime::attachments::Guard>,
    fs: FsContext,
    parent: Result<Option<Agent>>,
    operations: Vec<OperationInput>,
    resumes: HashMap<String, Result<Agent>>,
    sources: HashMap<String, Result<Source>>,
}
pub(in crate::runtime) struct SpawnFacts {
    pub source: Option<Result<Source>>,
    pub resolved: Result<crate::config::ResolvedSpawn>,
    pub context: Option<Result<SpawnContext>>,
    pub members: Vec<MemberFacts>,
    pub parent_options: Option<(String, crate::runtime::domain::OptionsSelection)>,
}
struct CatalogDiscovery {
    key: String,
    index: usize,
    input: BindingInput,
}
struct ResumeFacts {
    agent: Result<Identity>,
    checked: Result<()>,
}
#[derive(Default)]
pub(in crate::runtime) struct Facts {
    pub(in crate::runtime) attachment_guard: Option<crate::runtime::attachments::Guard>,
    attachments: HashMap<String, Result<Vec<crate::runtime::attachments::Attachment>>>,
    attachment_publication_error: Option<Error>,
    pub attachment_publication: Option<crate::runtime::attachments::Publication>,
    pub(in crate::runtime) spawns: HashMap<String, SpawnFacts>,
    discovery: Vec<CatalogDiscovery>,
    resumes: HashMap<String, ResumeFacts>,
    pub auth: Option<crate::runtime::preflight::auth::Prepared>,
    pub model_catalog: Option<Vec<crate::runtime::provider::protocol::Model>>,
    pub model_key: Option<crate::state::KeyAuthority>,
}
pub(in crate::runtime) fn missing() -> Error {
    Error::internal_error("Request filesystem preflight facts are missing.")
}
impl Facts {
    pub(in crate::runtime) async fn discover(
        &mut self,
        catalogs: &Arc<crate::runtime::provider::catalogs::Catalogs>,
    ) {
        for CatalogDiscovery { key, index, input } in std::mem::take(&mut self.discovery) {
            let result = catalogs
                .lookup_binding(input)
                .await
                .map(|(models, _)| models);
            if let Some(facts) = self.spawns.get_mut(&key) {
                facts.members[index].catalog = Some(result);
            }
        }
    }
    pub(in crate::runtime) fn tickets(&self) -> Vec<Arc<crate::state::RuntimeLease>> {
        self.spawns
            .values()
            .flat_map(|s| s.members.iter().filter_map(|m| m.lease.clone()))
            .collect()
    }
    pub(in crate::runtime) fn spawn(&self, envelope: &RequestEnvelope) -> Result<&SpawnFacts> {
        self.spawns.get(&envelope.fingerprint()).ok_or_else(missing)
    }
    pub(in crate::runtime) fn resume(&self, agent: &Agent) -> Result<()> {
        self.resumes
            .get(&agent.id)
            .ok_or_else(missing)?
            .checked
            .clone()
    }
    pub(in crate::runtime) fn fresh(&self, core: &Core) -> Result<bool> {
        for spawn in self.spawns.values() {
            if !spawn.members.iter().all(MemberFacts::fresh) {
                return Ok(false);
            }
            if let Some((id, selection)) = &spawn.parent_options {
                match core.store().agent(id) {
                    Ok(current)
                        if &crate::runtime::branching::effective_selection(&current)
                            == selection => {}
                    Ok(_) => return Ok(false),
                    Err(error) if missing_agent(&error) => return Ok(false),
                    Err(error) => return Err(error),
                }
            }
            // Previously captured validation failures remain deferred to dispatch.
            if let Some(Ok(source)) = &spawn.source {
                match core.store().agent(&source.agent.id) {
                    Ok(current) if source.fresh(&current) => {}
                    Ok(_) => return Ok(false),
                    Err(error) if missing_agent(&error) => return Ok(false),
                    Err(error) => return Err(error),
                }
            }
        }
        for (id, facts) in &self.resumes {
            if let Err(error) = &facts.agent
                && !missing_agent(error)
            {
                return Err(error.clone());
            }
            let fresh = match (&facts.agent, core.store().agent(id)) {
                (_, Err(error)) if !missing_agent(&error) => return Err(error),
                (Ok(proof), Ok(agent)) => *proof == Identity::from(&agent),
                (Err(old), Err(current)) => old.kind() == current.kind(),
                _ => false,
            };
            if !fresh {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

fn missing_agent(error: &Error) -> bool {
    matches!(
        error,
        Error::UnknownAgent { .. } | Error::AgentPurged { .. }
    )
}
struct OperationInput {
    key: String,
    request: Request,
}
fn operations(
    core: &Core,
    caller: &Caller,
    envelope: &RequestEnvelope,
    out: &mut Vec<OperationInput>,
) {
    match &envelope.request {
        Request::Spawn(_) | Request::Resume(_) => {
            out.push(OperationInput {
                key: envelope.fingerprint(),
                request: envelope.request.clone(),
            });
        }
        Request::Send(args) if attachment_preflight(&args.attachments) => {
            out.push(OperationInput {
                key: envelope.fingerprint(),
                request: envelope.request.clone(),
            });
        }
        Request::Batch(_) => {
            // Invalid batches fail at the dispatcher without needing external facts.
            if let Some(items) = envelope.batch_items() {
                for item in items {
                    if let Ok(item) = item.operation() {
                        // A replay or conflict is resolved lazily by dispatch. Neither
                        // needs to wait for external files again.
                        if matches!(core.replay(caller, &item.id, &item.request), Ok(None)) {
                            operations(core, caller, &item.request, out);
                        }
                    }
                }
            }
        }
        _ => {}
    }
}
pub(in crate::runtime) fn needed(envelope: &RequestEnvelope) -> bool {
    match &envelope.request {
        Request::Spawn(_) | Request::Resume(_) => true,
        Request::Send(args) => attachment_preflight(&args.attachments),
        Request::Batch(_) => envelope.batch_items().is_some_and(|items| {
            items
                .iter()
                .any(|item| item.operation().is_ok_and(|item| needed(&item.request)))
        }),
        _ => false,
    }
}

fn capture_source(core: &Core, id: &str) -> Result<Source> {
    let agent = core.store().agent(id)?;
    crate::runtime::features::require(&agent)?;
    let instructions = core.store().instructions(id)?;
    Ok(Source {
        agent,
        instructions,
    })
}

impl RequestInput {
    pub(in crate::runtime) fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }
    pub(in crate::runtime) fn capture(
        core: &Core,
        caller: &Caller,
        envelope: &RequestEnvelope,
    ) -> Self {
        let mut operations_list = Vec::new();
        operations(core, caller, envelope, &mut operations_list);
        let resumes = operations_list
            .iter()
            .filter_map(|operation| match &operation.request {
                Request::Resume(args) => args
                    .target
                    .as_str()
                    .map(|id| (id.to_owned(), core.store().agent(id))),
                _ => None,
            })
            .collect();
        let sources = operations_list
            .iter()
            .filter_map(|operation| match &operation.request {
                Request::Spawn(args) => args
                    .from_agent
                    .as_str()
                    .map(|id| (id.to_owned(), capture_source(core, id))),
                _ => None,
            })
            .collect();
        Self {
            attachment_root: core.store().attachments_dir.clone(),
            attachment_guard: None,
            fs: core.fs_context(),
            parent: caller
                .agent_id()
                .map(|id| core.store().agent(id))
                .transpose(),
            operations: operations_list,
            resumes,
            sources,
        }
    }
    pub(in crate::runtime) fn resolve(mut self) -> Facts {
        let mut publication = None;
        let mut facts = Facts {
            attachments: self.capture_attachments(&mut publication),
            attachment_guard: self.attachment_guard.clone(),
            ..Facts::default()
        };
        facts.attachment_publication = publication;
        facts.attachment_publication_error = self.operations.iter().find_map(|op| {
            facts
                .attachments
                .get(&op.key)
                .and_then(|result| result.as_ref().err())
                .cloned()
        });
        // Filesystem checks and lease acquisition retain request order and deferred errors.
        for OperationInput { key, request } in std::mem::take(&mut self.operations) {
            if let Request::Spawn(args) = request {
                let mut spawn = self.spawn_inputs(&args);
                let mut members = Vec::new();
                if let Ok(resolved) = &spawn.resolved
                    && spawn.context.as_ref().is_some_and(Result::is_ok)
                {
                    let pinned = spawn.source.as_ref().and_then(|s| s.as_ref().ok());
                    let names = match pinned.map_or_else(
                        || self.fs.config.auth_members(&resolved.auth),
                        |s| Ok(vec![s.agent.auth.clone()]),
                    ) {
                        Ok(names) => names,
                        Err(error) => {
                            // Preserve the validation error for normal dispatch;
                            // replayed requests still bypass their unused facts.
                            spawn.context = Some(Err(error));
                            facts.spawns.insert(key, spawn);
                            continue;
                        }
                    };
                    for auth in names {
                        let purpose = MemberPurpose::Spawn {
                            generation: pinned.map(|source| source.agent.auth_generation),
                        };
                        let (member, discovery) = self.fs.member(auth, purpose);
                        if let Some(input) = discovery {
                            facts.discovery.push(CatalogDiscovery {
                                key: key.clone(),
                                index: members.len(),
                                input,
                            });
                        }
                        members.push(member);
                    }
                }
                spawn.members = members;
                facts.spawns.insert(key, spawn);
            }
        }
        for (id, saved) in self.resumes {
            let agent = saved.as_ref().map(Identity::from).map_err(Clone::clone);
            let checked = saved.and_then(|agent| {
                crate::runtime::features::require(&agent)?;
                let binding =
                    self.fs
                        .state
                        .generation(&agent.auth, &agent.profile, agent.auth_generation)?;
                if !agent.conversation_clean {
                    self.fs
                        .state
                        .check_conversation(&binding, agent.provider_id())?;
                }
                Ok(())
            });
            facts.resumes.insert(id, ResumeFacts { agent, checked });
        }
        facts
    }
    fn spawn_inputs(&self, args: &SpawnArgs) -> SpawnFacts {
        let source = args.from_agent.as_str().map(|id| {
            self.sources
                .get(id)
                .cloned()
                .unwrap_or_else(|| Err(missing()))
        });
        let resolved = if let Some(source) = &source {
            source
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|source| source.resolve(&self.fs.config, args))
        } else {
            self.parent
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|parent| match parent.as_ref() {
                    Some(parent) => args.resolve_parent(&self.fs.config, Some(parent)),
                    None => args.resolve(&self.fs.config),
                })
        };
        let context = resolved.as_ref().ok().map(|resolved| {
            let parent = self.parent.as_ref().map_err(Clone::clone)?;
            Core::check_lifetime(resolved.lifetime, parent.is_some())?;
            self.context(
                args,
                resolved,
                source.as_ref().and_then(|s| s.as_ref().ok()),
            )
        });
        let parent_options = if source.is_none() {
            self.parent
                .as_ref()
                .ok()
                .and_then(Option::as_ref)
                .filter(|parent| {
                    resolved
                        .as_ref()
                        .is_ok_and(|resolved| resolved.provider == parent.profile.provider)
                })
                .map(|parent| {
                    (
                        parent.id.clone(),
                        crate::runtime::branching::effective_selection(parent),
                    )
                })
        } else {
            None
        };
        SpawnFacts {
            parent_options,
            source,
            resolved,
            context,
            members: vec![],
        }
    }
    fn context(
        &self,
        args: &SpawnArgs,
        resolved: &crate::config::ResolvedSpawn,
        source: Option<&Source>,
    ) -> Result<SpawnContext> {
        let parent = self.parent.as_ref().map_err(Clone::clone)?;
        let cwd = args
            .cwd
            .optional(Op::Spawn, "/cwd", "an absolute directory path string")?
            .map(std::path::PathBuf::as_path)
            .or_else(|| source.map(|s| s.agent.cwd.as_path()))
            .or_else(|| parent.as_ref().map(|p| p.cwd.as_path()))
            .ok_or_else(|| {
                Op::Spawn.error(
                    hive_input::Code::MissingArgument,
                    "/cwd",
                    "an absolute directory path when no source or parent supplies one",
                )
            })?;
        let cwd = super::context::directory(cwd, !args.cwd.is_missing())?;
        self.fs.launcher.check_workspace(&cwd)?;
        let instructions = super::context::instructions(&self.fs, args, resolved, source)?;
        args.body.required(Op::Spawn, "/body")?;
        Ok(SpawnContext { cwd, instructions })
    }
}

impl Facts {
    pub(in crate::runtime) fn attachments_published(&self) -> Result<()> {
        self.attachment_publication_error
            .clone()
            .map_or(Ok(()), Err)
    }
    pub(in crate::runtime) fn attachments(
        &self,
        envelope: &RequestEnvelope,
    ) -> Result<&[crate::runtime::attachments::Attachment]> {
        let Some(paths) = envelope.request.attachment_paths() else {
            return Ok(&[]);
        };
        if paths
            .optional(
                envelope.request.attachment_context(),
                "/attachments",
                "an array of absolute path strings",
            )?
            .is_none_or(Vec::is_empty)
        {
            return Ok(&[]);
        }
        self.attachments
            .get(&envelope.fingerprint())
            .ok_or_else(missing)?
            .as_deref()
            .map_err(Clone::clone)
    }
}
impl RequestInput {
    #[cfg(not(feature = "comb"))]
    fn attachment_capability(&self, request: &Request) -> Result<()> {
        if let Request::Spawn(args) = request {
            if let Some(id) = args.from_agent.as_str() {
                let source = self.sources.get(id).ok_or_else(missing)?;
                source
                    .as_ref()
                    .map_err(Clone::clone)?
                    .resolve(&self.fs.config, args)?;
            } else {
                args.resolve(&self.fs.config)?;
            }
        }
        Ok(())
    }
    pub(in crate::runtime) fn has_attachments(&self) -> bool {
        self.operations.iter().any(|op| {
            #[cfg(not(feature = "comb"))]
            if self.attachment_capability(&op.request).is_err() {
                return false;
            }
            op.request.attachment_paths().is_some_and(|paths| {
                !paths.is_missing()
                    && !matches!(paths, crate::runtime::api::Input::Valid(p) if p.is_empty())
            })
        })
    }
    fn capture_attachments(
        &self,
        publication: &mut Option<crate::runtime::attachments::Publication>,
    ) -> HashMap<String, Result<Vec<crate::runtime::attachments::Attachment>>> {
        if !self.has_attachments() {
            return HashMap::new();
        }
        let mut capture = crate::runtime::attachments::Capture::new(
            self.attachment_root.clone(),
            self.fs.state.root.clone(),
        );
        let mut results = HashMap::new();
        for op in &self.operations {
            #[cfg(not(feature = "comb"))]
            if self.attachment_capability(&op.request).is_err() {
                continue;
            }
            let Some(paths) = op.request.attachment_paths() else {
                continue;
            };
            if paths.is_missing() {
                continue;
            }
            let value = paths
                .optional(
                    op.request.attachment_context(),
                    "/attachments",
                    "an array of absolute path strings",
                )
                .and_then(|paths| {
                    let paths = paths.map_or(&[][..], Vec::as_slice);
                    capture.as_mut().map_err(|e| e.clone())?.message(paths)
                });
            results.insert(op.key.clone(), value);
        }
        if let (Ok(capture), Some(guard)) = (&capture, &self.attachment_guard) {
            *publication = Some(crate::runtime::attachments::Publication {
                root: self.attachment_root.clone(),
                files: capture.candidates(),
                guard: guard.clone(),
            });
        }
        // A failed capture publishes nothing, including earlier files in that batch.
        if results.values().any(Result::is_err) {
            // Preserve each operation's facts: sequential dispatch finds the first
            // error and rolls the whole batch back, without any published bytes.
            return results;
        }
        let publication = capture.and_then(crate::runtime::attachments::Capture::publish);
        if let Err(error) = publication {
            for value in results.values_mut().filter(|v| v.is_ok()) {
                *value = Err(error.clone());
            }
        }
        results
    }
}

fn attachment_preflight(paths: &crate::runtime::api::Input<Vec<std::path::PathBuf>>) -> bool {
    !paths.is_missing() && !matches!(paths, crate::runtime::api::Input::Valid(p) if p.is_empty())
}
