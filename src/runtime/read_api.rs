//! The same validation, pagination and decoding for transactional and control reads.
mod filters;
use super::{
    api::{self, Request},
    invalid_request,
    store::Queries,
};
use crate::error::Result;
use crate::runtime::api::InputContext as Op;
use api::pagination::{bytes, number, rows};
use serde_json::Value;

#[derive(Clone)]
pub(super) struct VerificationContext {
    config: String,
    root: String,
    binary: String,
}
impl VerificationContext {
    pub(super) fn new(
        config: &std::path::Path,
        root: &std::path::Path,
        binary: &std::path::Path,
    ) -> Result<Self> {
        fn absolute(path: &std::path::Path) -> Result<String> {
            std::path::absolute(path)
                .map_err(crate::error::Error::state_io_from)?
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| {
                    crate::error::Error::invalid_path("Verification argv requires UTF-8 paths.")
                })
        }
        Ok(Self {
            config: absolute(config)?,
            root: absolute(root)?,
            binary: absolute(binary)?,
        })
    }
    fn argv(&self, auth: &str, model: &str, effort: &str) -> Vec<String> {
        vec![
            "hive".into(),
            "--config".into(),
            self.config.clone(),
            "--state-dir".into(),
            self.root.clone(),
            "--claude-bin".into(),
            self.binary.clone(),
            "verify-steering".into(),
            "--auth".into(),
            auth.into(),
            "--model".into(),
            model.into(),
            "--effort".into(),
            effort.into(),
        ]
    }
}

// Agent lists use the largest public page; history reads use its smaller default.
const AGENT_PAGE_ROWS: u64 = super::limits::MAX_PAGE_ROWS;
const HISTORY_PAGE_ROWS: u64 = super::limits::DEFAULT_HISTORY_PAGE_ROWS;
// A tree reply carries at most this many agents and reports truncation.
pub(super) const TREE_ROWS: usize = 1000;
// Output pages share the control-plane retained-text cap.
const OUTPUT_BYTES: u64 = crate::limits::CONTROL_INPUT_BYTES as u64;

pub(super) fn dispatch(
    queries: &Queries,
    request: &Request,
    verification: Option<&VerificationContext>,
    config: &crate::config::Config,
) -> Result<Value> {
    let page_error = |error: api::pagination::InvalidPage| error.for_request(request);
    match request {
        #[cfg(feature = "comb")]
        Request::Collectors => queries.collector_status().map(|destinations| {
            api::value(api::CollectorReply {
                collectors: destinations,
            })
        }),
        Request::OptionsCatalog(args) => crate::options::listing(
            args.provider
                .optional(Op::OptionsCatalog, "/provider", "codex or claude")?
                .copied(),
            args.model
                .optional(Op::OptionsCatalog, "/model", "a model token")?
                .map(String::as_str),
        ),
        Request::AuthGroups => queries.auth_groups(config).map(api::value),
        Request::Roles => Ok(api::value(api::RolesResponse::new(config))),
        Request::AuthAvailability(args) => queries
            .auth_status(args.auth.required(Op::AuthAvailability, "/auth")?)
            .map(api::value),
        Request::Agents(args) if args.stalled_requested() => {
            rows(number(&args.limit, HISTORY_PAGE_ROWS, "/limit").map_err(page_error)?)
                .map_err(page_error)?;
            queries
                .stalled_groups(args, crate::state::now())
                .map(api::value)
        }
        Request::Agents(args) => filters::agents(queries, args, config),
        Request::Status(args) => status(queries, args, verification),
        Request::Tree(args) => tree(queries, args),
        Request::Attach(args) => attach(queries, args, verification),
        Request::Read(args) => history(queries, args, request),
        Request::Messages(args) => queries.messages(
            args.target.required(Op::Messages, "/target")?,
            match &args.before {
                api::Input::Missing | api::Input::Invalid(Value::Null) => None,
                value => Some(number(value, 0, "/before").map_err(page_error)?),
            },
            rows(number(&args.limit, HISTORY_PAGE_ROWS, "/limit").map_err(page_error)?)
                .map_err(page_error)?,
        ),
        Request::MessageResult(args) => queries
            .message_result(args.message_id.required(Op::MessageResult, "/message_id")?)
            .map(api::value),
        Request::Approvals(args) => queries.approvals(args).map(api::value),
        Request::Approval(args) => queries
            .approval(api::ApprovalInput::Show.id(&args.approval_id)?)
            .map(|approval| api::value(crate::approvals::ApprovalResponse { approval })),
        Request::Output(args) => queries.output(
            args.id.required(Op::Output, "/id")?,
            number(&args.offset, 0, "/offset").map_err(page_error)?,
            usize::try_from(
                bytes(number(&args.limit, OUTPUT_BYTES, "/limit").map_err(page_error)?)
                    .map_err(page_error)?,
            )
            .map_err(|cause| invalid_request().with_source(cause))?,
        ),
        _ => Err(invalid_request()),
    }
}

fn status(
    queries: &Queries,
    args: &api::StatusArgs,
    verification: Option<&VerificationContext>,
) -> Result<Value> {
    let mut agent = queries.agent(args.target.required(Op::Status, "/target")?)?;
    if args
        .instructions
        .optional(Op::Status, "/instructions", "a boolean")?
        .copied()
        == Some(true)
    {
        agent.instructions = queries.instructions(&agent.id)?;
    }
    let steering_verification = verification_targets(&agent, verification);
    let approval_id = queries.pending_approval(&agent.id)?;
    let mut branch = queries.branch_intent(&agent.id)?;
    if let Some(branch) = &mut branch {
        branch.source_handoff = None;
    }
    let branch_dependents = queries.branch_dependents(&agent.id)?;
    let agent = ApprovalStatus {
        open_joins: queries.open_joins(&agent.id)?,
        listing: api::ListingFields {
            held_for_build: queries.agent_hold(&agent.id)?,
            idle_since: queries.idle_since(&agent.id)?,
            provider: agent.profile.provider,
            billing: agent.profile.method,
            waiting_on: approval_id.as_ref().map(|_| "human"),
            approval_id: approval_id.as_deref(),
        },
        agent: agent.public(),
        branch,
        branch_dependents,
    };
    Ok(api::value(api::AgentResponse {
        attachment_store_bytes: queries.attachment_bytes()?,
        latest_attachments: queries
            .latest_attachments(args.target.required(Op::Status, "/target")?)?,
        agent,
        steering_verification,
    }))
}

/// Compact rows of one subtree, or of the live roots, from one snapshot.
fn tree(queries: &Queries, args: &api::TreeArgs) -> Result<Value> {
    let root = args
        .root
        .optional(Op::Tree, "/root", "an agent ID")?
        .map(String::as_str);
    let all = args
        .all
        .optional(Op::Tree, "/all", "a boolean")?
        .copied()
        .unwrap_or(false);
    if all && root.is_some() {
        return Err(Op::Tree.error(
            hive_input::Code::ConflictingArguments,
            "/all",
            "either root or all, not both",
        ));
    }
    let ids = if all {
        queries.live_ids(TREE_ROWS)?
    } else {
        queries.tree_ids(root, TREE_ROWS)?
    };
    let truncated = ids.len() > TREE_ROWS;
    let agents = ids
        .iter()
        .take(TREE_ROWS)
        .map(|id| queries.tree_row(&queries.agent(id)?))
        .collect::<Result<_>>()?;
    Ok(api::value(api::TreeResponse { agents, truncated }))
}

/// The agent's status, newest event page and counts, from one snapshot.
fn attach(
    queries: &Queries,
    args: &api::AttachArgs,
    verification: Option<&VerificationContext>,
) -> Result<Value> {
    let target = args.target.required(Op::Attach, "/target")?;
    let invalid = |error: api::pagination::InvalidPage| {
        Op::Attach.error(error.code, error.field, error.expected)
    };
    let limit = rows(number(&args.limit, HISTORY_PAGE_ROWS, "/limit").map_err(invalid)?)
        .map_err(invalid)?;
    let cursor = |value: &api::Input<u64>, field| match value {
        api::Input::Missing => Ok(None),
        value => number(value, 0, field).map(Some).map_err(invalid),
    };
    let (after, before) = (
        cursor(&args.after, "/after")?,
        cursor(&args.before, "/before")?,
    );
    if after.is_some() && before.is_some() {
        return Err(Op::Attach.error(
            hive_input::Code::ConflictingArguments,
            "/before",
            "either after or before, not both",
        ));
    }
    if before == Some(0) {
        return Err(Op::Attach.error(
            hive_input::Code::OutOfRange,
            "/before",
            "an event sequence of at least 1",
        ));
    }
    let status = status(
        queries,
        &api::StatusArgs {
            target: args.target.clone(),
            ..api::StatusArgs::default()
        },
        verification,
    )?;
    let events = match (after, before) {
        (_, Some(before)) => queries.events_before(target, before, limit)?,
        (Some(after), None) => queries.events(target, after, limit)?,
        (None, None) => queries.events(target, queries.tail_after(target, limit)?, limit)?,
    };
    let counts = queries.counts(target)?;
    Ok(api::value(api::AttachResponse {
        agent: status["agent"].clone(),
        events,
        queued: counts.queued,
        mail: counts.mail,
        children: counts.children,
    }))
}

// Artifacts are immutable until explicit purge; its reader barrier precedes unlink.
// Their manifest SELECT ends before file I/O; no WAL snapshot spans a file read.
pub(super) fn artifact(request: &Request) -> bool {
    matches!(request, Request::Output(_))
        || matches!(request, Request::Read(args) if args.target.as_str()
            .is_some_and(|target| target.starts_with("artifact:")))
}

pub(super) fn ping(
    binary_sha256: String,
    provider_selection: super::ProviderSelection,
    storage: api::StorageDiagnostics,
    resident_actors: usize,
    profile_leases: usize,
    writer_observation: api::WriterObservation,
) -> api::PingResponse {
    api::PingResponse {
        binary_sha256,
        commit: hive_setup_support::identity::COMMIT,
        protocols: crate::compatibility::CONTROL_RANGE,
        agent_catalog: crate::compatibility::AGENT_CATALOG.current,
        state_format: crate::compatibility::STATE.current,
        provider_compatibility: crate::provider_support::REGISTRIES,
        provider_selection,
        configuration: None,
        deprecations: Vec::new(),
        managed_service: hive_setup_support::managed::label(),
        product_version: env!("CARGO_PKG_VERSION"),
        compiled_features: super::features::COMPILED,
        comb_backlog: None,
        status: "running",
        pid: std::process::id(),
        storage,
        resident_actors,
        profile_leases,
        writer_observation,
        capabilities: api::Capabilities {
            providers: [
                api::ProviderCapabilities {
                    provider: "codex",
                    subscription: true,
                    next_turn: true,
                    redirect: true,
                    steering_policy: api::SteeringPolicy::NativeProbe,
                    read_only_shell: true,
                },
                api::ProviderCapabilities {
                    provider: "claude",
                    subscription: true,
                    next_turn: true,
                    redirect: true,
                    steering_policy: api::SteeringPolicy::VerifiedCanary,
                    read_only_shell: false,
                },
            ],
            approvals: concat!(
                "advisory asynchronous human questions; ",
                "native provider approvals remain disabled"
            ),
        },
    }
}

#[derive(serde::Serialize)]
struct ApprovalStatus<'a> {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    open_joins: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<super::branching::Intent>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    branch_dependents: Vec<String>,
    #[serde(flatten)]
    listing: api::ListingFields<'a>,
    #[serde(flatten)]
    agent: super::domain::Agent<
        super::domain::CombCapability,
        Vec<String>,
        crate::options::OptionsView,
    >,
}

fn verification_targets(
    agent: &super::domain::Agent,
    context: Option<&VerificationContext>,
) -> Vec<api::SteeringVerification> {
    use super::domain::AgentState;
    if agent.profile.provider != crate::config::Provider::Claude || agent.steering.supported() {
        return vec![];
    }
    let target = |selection: super::domain::OptionsSelection, source| api::SteeringVerification {
        argv: context.map(|context| context.argv(&agent.auth, &selection.model, &selection.effort)),
        auth: agent.auth.clone(),
        model: selection.model,
        effort: selection.effort,
        selection: source,
        daemon_must_be_stopped: true,
        enables_next_launch: true,
    };
    let mut targets = Vec::new();
    let next = agent.boundary_selection();
    if matches!(agent.state, AgentState::Idle | AgentState::Running)
        && let Some(session) = &agent.session_selection
    {
        targets.push(target(
            session.clone(),
            api::VerificationSelection::CurrentSession,
        ));
        if session.model == next.model && session.effort == next.effort {
            return targets;
        }
    }
    targets.push(target(next, api::VerificationSelection::NextTurn));
    targets
}

fn history(queries: &Queries, args: &api::ReadArgs, request: &Request) -> Result<Value> {
    let page_error = |error: api::pagination::InvalidPage| error.for_request(request);
    let target = args.target.required(Op::Read, "/target")?;
    let page = args.page().map_err(page_error)?;
    if let Some(join) = target.strip_prefix("join:") {
        queries
            .find_join_snapshot(join)?
            .map(api::value)
            .ok_or_else(|| {
                Op::ReadJoin.error(
                    hive_input::Code::InvalidValue,
                    "/target",
                    "a currently retained join",
                )
            })
    } else {
        queries.events(target, page.after, page.limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn verification_argv_uses_absolute_daemon_context_and_reported_selection() {
        use std::path::Path;
        let context = VerificationContext::new(
            Path::new("daemon config/auth.yaml"),
            Path::new("relative daemon state"),
            Path::new("native claude"),
        )
        .expect("paths");
        let mut agent = crate::runtime::test_support::agent();
        agent.profile.provider = crate::config::Provider::Claude;
        agent.session_selection = Some(agent.desired_selection());
        agent.model = "newer-desired-model".into();
        let targets = verification_targets(&agent, Some(&context));
        let argv = targets[0].argv.as_ref().expect("daemon argv");
        assert_eq!(
            argv[2],
            std::path::absolute("daemon config/auth.yaml")
                .expect("absolute config")
                .to_str()
                .expect("utf8")
        );
        assert_eq!(
            argv[4],
            std::path::absolute("relative daemon state")
                .expect("absolute state")
                .to_str()
                .expect("utf8")
        );
        assert_eq!(
            argv[6],
            std::path::absolute("native claude")
                .expect("absolute binary")
                .to_str()
                .expect("utf8")
        );
        assert_eq!(argv[11], "gpt-5.6-luna");
        assert_eq!(
            targets[1].argv.as_ref().expect("next argv")[11],
            "newer-desired-model"
        );
    }
    #[test]
    fn verification_argv_never_replaces_non_utf8_path_bytes() {
        use std::{ffi::OsStr, os::unix::ffi::OsStrExt, path::Path};
        assert!(
            VerificationContext::new(
                Path::new("/config"),
                Path::new("/state"),
                Path::new(OsStr::from_bytes(b"/native-\xff"))
            )
            .is_err()
        );
    }
    #[test]
    fn verification_targets_distinguish_current_session_and_next_selection() {
        let mut agent = crate::runtime::test_support::agent();
        agent.profile.provider = crate::config::Provider::Claude;
        agent.session_selection = Some(agent.desired_selection());
        agent.model = "new-model".into();
        let targets = verification_targets(&agent, None);
        assert_eq!(targets.len(), 2);
        assert!(targets[0].selection == api::VerificationSelection::CurrentSession);
        assert_eq!(targets[0].model, "gpt-5.6-luna");
        assert!(targets[1].selection == api::VerificationSelection::NextTurn);
        assert_eq!(targets[1].model, "new-model");
        agent.steering = crate::runtime::domain::SteeringCapability::Supported;
        assert!(verification_targets(&agent, None).is_empty());
    }
}
