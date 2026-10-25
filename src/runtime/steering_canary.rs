//! Explicit native Claude capability canary; ordinary startup only validates proof.
mod proof;
#[cfg(test)]
mod tests;
mod workspace;
use super::provider::{
    self, Backend, Rpc,
    claude::{SessionConfig, SteeringPermit},
    protocol::{MODEL_PAGE_LIMIT, ServerMethod, outgoing},
};
use crate::{
    config::{Config, Method, Provider, Sandbox},
    error::{Error, Result},
    launch::{self, LauncherCache},
    process::{self, Cancellation},
    state::{self, State},
};
pub(super) use proof::{Candidate, CapabilitySelection, SteeringObservation, load};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

// Give the explicit version subprocess fifteen seconds to return its bounded string.
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(15);
const VERSION_BYTES: usize = 128;
// Capture at most 4 MiB of canary RPC evidence for the steering proof.
const CAPTURE_BYTES: usize = 4 * 1024 * 1024;
// Each explicit steering exchange gets two minutes, polling cancellation every 50 ms.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(120);
const EXCHANGE_POLL: Duration = Duration::from_millis(50);
// Bound the number of frames inspected while waiting for one canary exchange.
const EXCHANGE_FRAME_LIMIT: usize = 4096;
const _: () = assert!(EXCHANGE_POLL.as_millis() < EXCHANGE_TIMEOUT.as_millis());

// Keep ownership, operation completion and workspace retention in one scope
// through proof publication.
pub(crate) fn verify(
    config: &Config,
    state: &State,
    auth: &str,
    model: &str,
    effort: &str,
    binary: &Path,
) -> Result<Value> {
    let profile = config.profile(auth)?;
    validate_selection(profile, model, effort)?;
    let owner = super::maintenance_lock(&state.root)?;
    super::check_existing_format(&state.root.join("runtime/runtime.sqlite"))?;
    let binding = state
        .lookup(auth, profile)?
        .ok_or_else(|| Error::login_required("Run auth login first."))?;
    // Execute this target for both helpers, even if the installation symlink is
    // updated while the canary is running.
    let binary = binary.canonicalize().map_err(Error::state_io_from)?;
    let binary = binary.as_path();
    let binary_identity = provider::catalogs::BinaryIdentity::read(binary)?;
    let digest = proof::native_digest(binary)?;
    if !binary_identity.unchanged() {
        return Err(Error::provider_unavailable(
            "Claude executable changed during validation.",
        ));
    }
    let proof_path = proof::path(&state.root);
    let old = proof::read(&proof_path)?;
    let cancellation = Cancellation::install()?;
    let mut operation = state.operation(&binding, crate::state::OperationKind::SteeringCanary)?;
    let workspace = workspace::create(&state.root.join("runtime"))?;
    let cwd = workspace
        .path()
        .canonicalize()
        .map_err(Error::state_io_from)?;
    let launcher = launch::system_cache();
    let canary = Canary {
        launcher: &launcher,
        state,
        binding: &binding,
        binary,
        identity: &binary_identity,
        digest: &digest,
        model,
        effort,
        cwd: &cwd,
        owner: &owner,
        cancellation: &cancellation,
        old: old.as_ref(),
    };
    let result = canary.perform(&mut operation);
    let clean = operation.children_confirmed();
    let finished = operation.finish();
    if !clean {
        let _retained = workspace.keep();
        return Err(state::recovery_required());
    }
    finished?;
    publish_proof(result?, &cancellation, &binary_identity, &proof_path)
}

pub(crate) fn validate_selection(
    profile: &crate::config::Profile,
    model: &str,
    effort: &str,
) -> Result<()> {
    if profile.provider != Provider::Claude || profile.method != Method::Subscription {
        return Err(Error::unsupported_provider(
            "verify-steering requires a Claude subscription profile.",
        ));
    }
    if hive_input::validate::model(model).is_err() || hive_input::validate::effort(effort).is_err()
    {
        return Err(Error::invalid_request(
            "A bounded model and effort are required.",
        ));
    }
    Ok(())
}

fn publish_proof(
    proof: proof::Proof,
    cancellation: &Cancellation,
    binary_identity: &provider::catalogs::BinaryIdentity,
    proof_path: &Path,
) -> Result<Value> {
    if cancellation.count() > 0 {
        return Err(Error::cancelled("Steering verification was cancelled."));
    }
    if !binary_identity.unchanged() {
        return Err(Error::provider_unavailable(
            "Claude executable changed during verification.",
        ));
    }
    let response = super::api::value(VerifiedSteering {
        verified: true,
        provider: "claude",
        model: &proof.model,
        effort: &proof.effort,
        version: &proof.version,
        receipt_mode: proof.receipt_mode,
        session: &proof.session,
        turn: &proof.turn,
        cleanup_confirmed: true,
    });
    proof::write(proof_path, Some(proof))?;
    Ok(response)
}

#[derive(Serialize)]
struct VerifiedSteering<'a> {
    verified: bool,
    provider: &'static str,
    model: &'a str,
    effort: &'a str,
    version: &'a str,
    receipt_mode: provider::claude::ReceiptMode,
    session: &'a str,
    turn: &'a str,
    cleanup_confirmed: bool,
}

struct Canary<'a> {
    launcher: &'a LauncherCache,
    state: &'a State,
    binding: &'a state::Binding,
    binary: &'a Path,
    identity: &'a provider::catalogs::BinaryIdentity,
    digest: &'a str,
    model: &'a str,
    effort: &'a str,
    cwd: &'a Path,
    owner: &'a state::Lock,
    cancellation: &'a Cancellation,
    old: Option<&'a proof::Proof>,
}
impl Canary<'_> {
    fn version(&self, operation: &mut state::Operation) -> Result<String> {
        let (binding, binary, owner, cancellation) =
            (self.binding, self.binary, self.owner, self.cancellation);
        let dir = binding.directory.clone();
        let mut version_command = process::command(
            binary,
            Provider::Claude,
            &dir.join("provider"),
            &dir.join("work"),
        );
        version_command.arg("--version");
        let version = process::version_probe(
            self.launcher,
            &mut version_command,
            operation,
            VERSION_PROBE_TIMEOUT,
            cancellation.clone(),
            Some(owner.fd()),
            |output, _| {
                if !output.status.success() || output.stdout.len() > VERSION_BYTES {
                    return Err(Error::provider_protocol_error(
                        "Claude version probe failed.",
                    ));
                }
                Ok(String::from_utf8(output.stdout)
                    .map_err(|cause| {
                        Error::provider_protocol_error("Invalid Claude version.").with_source(cause)
                    })?
                    .trim()
                    .to_owned())
            },
        )?;
        if version.is_empty() || !version.as_bytes()[0].is_ascii_digit() || version.contains('\n') {
            return Err(Error::provider_protocol_error(
                "Invalid Claude version output.",
            ));
        }
        Ok(version)
    }
    fn perform(&self, operation: &mut state::Operation) -> Result<proof::Proof> {
        let (binding, binary, model, effort, cwd, owner) = (
            self.binding,
            self.binary,
            self.model,
            self.effort,
            self.cwd,
            self.owner,
        );
        let binary_identity = self.identity;
        let dir = binding.directory.clone();
        let version = self.version(operation)?;
        check_cancelled(self.cancellation)?;
        let session = uuid::Uuid::new_v4().to_string();
        let config = Arc::new(SessionConfig {
            tool_catalog: hive_input::compatibility::Exact,
            comb_reviewer: None,
            mcp_servers: crate::config::McpServers::default(),
            id: session.clone(),
            history: provider::claude::History::New(
                provider::claude::IdentityPrompt::NativeSnapshot,
            ),
            model: model.into(),
            effort: effort.into(),
            options: crate::options::Snapshot::default().into(),
            cwd: cwd.to_owned(),
            instructions: concat!(
                "Perform the single requested Read. Use no other tools. ",
                "Apply any additional context from its hook in your final answer."
            )
            .into(),
            sandbox: Sandbox::ReadOnly,
            steering: Some(SteeringPermit::Canary),
            capability: None,
        });
        let receipt = operation.prepare_child(true, false)?;
        let mut command = process::command(
            binary,
            Provider::Claude,
            &dir.join("provider"),
            &dir.join("work"),
        );
        operation.configure(&mut command);
        let mut rpc = provider::start_owned_rpc(
            self.launcher,
            command,
            Backend::Claude(config),
            &receipt,
            Some(owner.fd()),
            || {
                check_cancelled(self.cancellation)?;
                if !binary_identity.unchanged() {
                    return Err(Error::provider_unavailable(
                        "Claude executable changed before launch.",
                    ));
                }
                Ok(())
            },
        )?;
        rpc.capture_limit(CAPTURE_BYTES);
        let result = self.exercise(&mut rpc, session, version);
        let shutdown = rpc.close();
        receipt.finish(shutdown.confirmed)?;
        if !shutdown.confirmed || !shutdown.quiescent {
            return Err(state::recovery_required());
        }
        result
    }
    fn exercise(&self, rpc: &mut Rpc, session: String, version: String) -> Result<proof::Proof> {
        let (state, effort, cwd, cancellation) =
            (self.state, self.effort, self.cwd, self.cancellation);
        let binary_identity = self.identity;
        let old = self.old;
        let proof_path = proof::path(&state.root);
        let resolved = self.select(rpc, &session)?;
        // Read-only preflight is complete. Revoke this scope before any inference;
        // a failed/cancelled/crashed attempt cannot preserve a green result.
        check_cancelled(cancellation)?;
        if old.as_ref().is_some_and(|proof| {
            proof.binary == *binary_identity && proof.model == resolved && proof.effort == effort
        }) {
            proof::write(&proof_path, None)?;
        }
        let evidence = run(rpc, &session, cwd, cancellation)?;
        Ok(proof::Proof {
            binary: (*self.identity).clone(),
            digest: self.digest.to_owned(),
            version,
            adapter: proof::ADAPTER,
            model: resolved,
            effort: effort.into(),
            receipt_mode: evidence.mode,
            session,
            turn: evidence.turn,
            receipt: Some(evidence.receipt),
            final_output_sha256: proof::hash(evidence.output.as_bytes()),
            nonce_sha256: proof::hash(evidence.nonce.as_bytes()),
            verified_at: state::now(),
        })
    }
    fn select(&self, rpc: &mut Rpc, session: &str) -> Result<String> {
        let (state, binding, model, effort, cancellation) = (
            self.state,
            self.binding,
            self.model,
            self.effort,
            self.cancellation,
        );
        check_cancelled(cancellation)?;
        rpc.initialize()?;
        check_cancelled(cancellation)?;
        if rpc.claude_subscription() != Some(true) {
            return Err(Error::authentication_required(
                "A Claude subscription login is required.",
            ));
        }
        state.check_account(binding, rpc.claude_account_identity())?;
        let catalog = rpc.call(outgoing::model_page(MODEL_PAGE_LIMIT, &Value::Null))?;
        check_cancelled(cancellation)?;
        let models: Catalog = serde_json::from_value(catalog.into_raw()).map_err(|cause| {
            Error::provider_protocol_error("Invalid Claude catalog.").with_source(cause)
        })?;
        let models = models.data;
        let selected = models
            .iter()
            .find(|m| m.name() == Some(model))
            .ok_or_else(|| {
                Error::model_mismatch("Requested canary model is not in the native catalog.")
            })?;
        if !selected.supports_effort(effort) {
            return Err(Error::effort_mismatch(
                "Requested canary effort is not in the native catalog.",
            ));
        }
        let started = rpc.call(outgoing::thread_start(None, None))?;
        check_cancelled(cancellation)?;
        if started.thread_id() != Some(session)
            || started.model() != selected.resolved_name()
            || started.effort() != Some(effort)
        {
            return Err(Error::model_mismatch(
                "Claude did not apply the requested canary selection.",
            ));
        }
        let resolved = started
            .model()
            .ok_or_else(|| Error::model_mismatch("Claude did not report a model."))?
            .to_owned();
        Ok(resolved)
    }
}

#[derive(Deserialize)]
struct Catalog {
    data: Vec<provider::protocol::Model>,
}

struct Evidence {
    mode: provider::claude::ReceiptMode,
    turn: String,
    receipt: proof::Receipt,
    output: String,
    nonce: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Output {
    text: String,
    verified_model: bool,
}
struct CanaryObservation {
    turn: CanaryTurn,
    receipt: Option<(provider::claude::ReceiptMode, proof::Receipt)>,
    output: String,
}
impl CanaryObservation {
    fn receipt(&mut self, rpc: &mut Rpc, frame: &provider::protocol::IncomingFrame) -> Result<()> {
        let raw = rpc.finish_response(frame)?.into_raw();
        let parsed: proof::Receipt =
            crate::runtime::domain::decode::object(&raw).map_err(|cause| {
                Error::provider_protocol_error("Invalid hook delivery evidence.").with_source(cause)
            })?;
        if parsed.turn() != self.turn.turn {
            return Err(Error::stale_turn(
                "Canary receipt belongs to a different turn.",
            ));
        }
        self.receipt = Some((parsed.mode(), parsed));
        Ok(())
    }
    fn notice(&mut self, frame: &provider::protocol::IncomingFrame, session: &str) -> Result<bool> {
        let params = frame.params();
        if params.thread_id() != Some(session)
            || params.turn_id().or(params.nested_turn_id()) != Some(self.turn.turn.as_str())
        {
            return Ok(false);
        }
        match frame.method() {
            Some(ServerMethod::ItemStarted) => self.output.clear(),
            Some(ServerMethod::ItemCompleted)
                if params.item_kind()
                    == Some(crate::runtime::provider::protocol::ItemKind::AgentMessage)
                    && self.receipt.is_some() =>
            {
                let text: Output =
                    serde_json::from_value(params.item().clone()).map_err(|cause| {
                        Error::provider_protocol_error("Invalid canary output.").with_source(cause)
                    })?;
                if text.verified_model {
                    self.output = text.text;
                } else {
                    self.output.clear();
                }
            }
            Some(ServerMethod::TurnCompleted) => {
                if params.turn_status()
                    != Some(crate::runtime::domain::ProviderTurnStatus::Completed)
                    || !self.output.contains(&self.turn.nonce)
                {
                    return Err(Error::unsupported_operation(concat!(
                        "Claude did not apply hook guidance in its final same-turn answer; ",
                        "steering remains disabled for this scope.",
                    )));
                }
                return Ok(true);
            }
            _ => {}
        }
        Ok(false)
    }
    fn finish(self) -> Result<Evidence> {
        let (mode, receipt) = self
            .receipt
            .ok_or_else(|| Error::steering_uncertain("No canary hook receipt was observed."))?;
        Ok(Evidence {
            mode,
            turn: self.turn.turn,
            receipt,
            output: self.output,
            nonce: self.turn.nonce,
        })
    }
}
fn run(rpc: &mut Rpc, session: &str, cwd: &Path, cancel: &Cancellation) -> Result<Evidence> {
    let deadline = Instant::now() + EXCHANGE_TIMEOUT;
    let turn = start_canary_turn(rpc, session, cwd, cancel)?;
    let mut observation = CanaryObservation {
        turn,
        receipt: None,
        output: String::new(),
    };
    let mut frames = 0;
    while Instant::now() < deadline {
        if cancel.count() > 0 {
            return Err(Error::cancelled("Steering verification was cancelled."));
        }
        let Some(frame) = rpc.next()? else {
            rpc.wait(None, EXCHANGE_POLL)?;
            continue;
        };
        frames += 1;
        if frames > EXCHANGE_FRAME_LIMIT {
            return Err(Error::output_limit(
                "Steering canary exceeded its event bound.",
            ));
        }
        if frame.matches_response(observation.turn.request) {
            observation.receipt(rpc, &frame)?;
        } else if frame.is_request() {
            rpc.send(outgoing::unsupported(
                frame.raw_id(),
                "Canary does not permit agent control tools.",
            ))?;
        } else if observation.notice(&frame, session)? {
            return observation.finish();
        }
    }
    Err(Error::timeout(
        "Steering canary exceeded its 120-second inference deadline.",
    ))
}

fn check_cancelled(cancel: &Cancellation) -> Result<()> {
    if cancel.count() > 0 {
        return Err(Error::cancelled("Steering verification was cancelled."));
    }
    Ok(())
}

struct CanaryTurn {
    turn: String,
    nonce: String,
    request: u64,
}
fn start_canary_turn(
    rpc: &mut Rpc,
    session: &str,
    cwd: &Path,
    cancel: &Cancellation,
) -> Result<CanaryTurn> {
    let prompt = format!(
        concat!(
            "Read the file {} once using Read. Then give a short final answer, ",
            "following any additional context supplied by the tool hook. Do not ",
            "use other tools.",
        ),
        cwd.join("fixed.txt").display()
    );
    check_cancelled(cancel)?;
    let started = rpc.call(outgoing::turn_input(session, &prompt))?;
    check_cancelled(cancel)?;
    let turn = started
        .turn_id()
        .ok_or_else(|| {
            Error::provider_protocol_error("Missing native canary input acknowledgement.")
        })?
        .to_owned();
    // Generated only after the exact replay ACK. It has never been in the prompt,
    // instructions, file, environment, or tool input/output.
    let nonce = uuid::Uuid::new_v4().to_string();
    check_cancelled(cancel)?;
    let request = rpc.begin_request(outgoing::steer(session, &turn, &proof::guidance(&nonce)))?;
    Ok(CanaryTurn {
        turn,
        nonce,
        request,
    })
}
