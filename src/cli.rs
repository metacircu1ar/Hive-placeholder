//! Command-line arguments and the process entrypoint.
mod execution;
use execution::run_parsed;

mod agent_options;
mod attach;
mod auth;
mod batch;
#[cfg(test)]
mod batch_tests;
mod catalog;
#[cfg(feature = "comb")]
mod collectors;
mod configuration;
mod daemon;
mod dispatch;
mod doctor;
mod hangup;
mod input;
mod join_wait;
mod live;
mod logging;
mod logs;
mod maintenance;
mod mouse;
mod observe;
mod panes;
mod parsing;
mod paths;
mod read;
mod remote;
mod requests;
mod send;
mod services;
mod setup;
mod tree_feed;
mod tui;
mod version;
mod views;
mod wait;

use auth::auth_command;
use daemon::daemon_command;
use input::batch_operations;
use logging::execute;
use remote::{models_command, remote_response};
#[cfg(test)]
use wait::{WaitUntil, Waiter};

use crate::runtime::api::{
    self, AgentsArgs, CompleteArgs, Input, MessagesArgs, OutputArgs, ReadArgs, Request, SendArgs,
    SetOptionsArgs, StatusArgs, TargetArgs,
};
use crate::{
    auth::{Auth, LoginOutcome, Settings},
    config::Config,
    error::{Error, Result},
    process::Cancellation,
    runtime::{
        self,
        domain::{AgentState, DeliveryMode, TaskOutcome},
    },
    state::State,
};
use clap::{Parser, Subcommand};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

mod approvals;
#[cfg(test)]
#[allow(clippy::unwrap_used)] // CLI fixtures deliberately panic on setup failures.
mod attachment_tests;
mod duration;
mod gc;
#[cfg(test)]
#[allow(clippy::unwrap_used)] // Stub fixtures and response assertions fail on setup errors.
mod response_tests;
mod spawn;

// Authentication probe deadline used when --probe-timeout is omitted.
const DEFAULT_PROBE_SECONDS: u64 = 15;
// Largest authentication probe deadline accepted by the CLI.
const MAX_PROBE_SECONDS: u64 = 300;
// Interactive authentication deadline used when --login-timeout is omitted.
const DEFAULT_LOGIN_SECONDS: u64 = 600;
// Largest interactive authentication deadline accepted by the CLI.
const MAX_LOGIN_SECONDS: u64 = 3600;
// Require CLI timeouts to allow at least one second of work.
const MIN_TIMEOUT_SECONDS: u64 = 1;
// Events retained per agent when compact omits --keep-per-agent.
const DEFAULT_KEEP_EVENTS: u64 = 100;
// Artifact byte count requested when output omits --limit.
const DEFAULT_OUTPUT_BYTES: u64 = crate::limits::CONTROL_INPUT_BYTES as u64;
// Agent-list page size requested when agents omits --limit.
const DEFAULT_AGENT_PAGE_ROWS: u64 = runtime::limits::MAX_PAGE_ROWS;
// Event page size requested when read omits --limit.
const DEFAULT_EVENT_PAGE_ROWS: u64 = runtime::limits::DEFAULT_HISTORY_PAGE_ROWS;
// Message page size requested when messages omits --limit.
const DEFAULT_MESSAGE_PAGE_ROWS: u64 = runtime::limits::DEFAULT_HISTORY_PAGE_ROWS;
// Wait deadline used when --timeout is omitted.
const DEFAULT_WAIT_SECONDS: u64 = 60;
// Largest wait deadline accepted by the CLI.
const MAX_WAIT_SECONDS: u64 = 3600;
// Maximum inline or file-backed message/result text.
const BODY_BYTES: usize = crate::limits::CONTROL_INPUT_BYTES;
// Maximum JSON batch input accepted from --operations-file.
const BATCH_BYTES: usize = runtime::limits::BATCH_INPUT_BYTES;
// Interval between CLI status checks while waiting for completion.
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(100);
// Maximum events inspected per page while waiting for turn completion.
const WAIT_EVENT_PAGE_ROWS: u64 = runtime::limits::MAX_PAGE_ROWS;
// How long daemon start waits for the control socket to answer.
const DAEMON_START_TIMEOUT: Duration = Duration::from_secs(10);
// Interval between control-socket readiness checks during daemon start.
const DAEMON_START_POLL: Duration = Duration::from_millis(50);
// How long restart waits for a stopping daemon to release its state. Provider
// shutdown can take several sequential 10 s graces (interrupt, unload, exit).
const DAEMON_STOP_TIMEOUT: Duration = Duration::from_secs(60);
// Operational failures are distinct from typed caller-input rejections (exit 2).
const EXIT_ERROR: i32 = 1;
const _: () = assert!(DEFAULT_PROBE_SECONDS <= MAX_PROBE_SECONDS);
const _: () = assert!(DEFAULT_LOGIN_SECONDS <= MAX_LOGIN_SECONDS);
const _: () = assert!(DEFAULT_WAIT_SECONDS <= MAX_WAIT_SECONDS);
const _: () = assert!(DAEMON_START_POLL.as_millis() < DAEMON_START_TIMEOUT.as_millis());

#[derive(Parser)]
#[command(
    name = "hivectl",
    version = crate::runtime::features::VERSION,
    about = "Run and coordinate persistent Codex and Claude agents"
)]
struct Cli {
    #[arg(skip)]
    explicit_globals: Vec<&'static str>,
    #[arg(skip)]
    explicit_provider: Option<&'static str>,
    #[arg(long, global = true, hide = true)]
    managed_service: Option<PathBuf>,
    #[arg(long, global = true)]
    #[doc = " Configuration for setup, services, daemon control, configured auth/model \
        commands and tui (whose tui.toml sits beside it)."]
    config: Option<PathBuf>,
    #[arg(long, global = true)]
    /// Controller state for all commands except version.
    state_dir: Option<PathBuf>,
    #[arg(long, global = true, default_value = "codex")]
    /// Provider for setup, doctor, auth probes, models, daemon run/start or service install.
    codex_bin: OsString,
    #[arg(long, global = true, default_value = "claude")]
    /// Provider for the same commands as --codex-bin, plus verify-steering.
    claude_bin: OsString,
    #[arg(
        long,
        global = true,
        default_value_t = DEFAULT_PROBE_SECONDS,
        value_parser = clap::value_parser!(u64).range(MIN_TIMEOUT_SECONDS..=MAX_PROBE_SECONDS)
    )]
    /// Probe deadline for setup, doctor and auth status/login/recover/repair.
    probe_timeout: u64,
    #[arg(
        long,
        global = true,
        default_value_t = DEFAULT_LOGIN_SECONDS,
        value_parser = clap::value_parser!(u64).range(MIN_TIMEOUT_SECONDS..=MAX_LOGIN_SECONDS)
    )]
    /// Login deadline for setup and auth login/repair.
    login_timeout: u64,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Read private daemon diagnostics as JSONL without contacting the daemon.
    Logs(logs::Logs),
    /// Configure providers, Comb, skills and per-user login services.
    Setup(Box<setup::Options>),
    #[command(hide = true)]
    SetupMaintenance(hive_setup_support::maintenance::Args),
    /// Install and control this configuration's per-user login service.
    Service {
        #[command(subcommand)]
        command: hive_setup_support::service::Command,
    },
    /// Check configuration, providers, login, daemon and Comb without inference.
    Doctor,
    /// Validate and maintain configuration without contacting providers or a daemon.
    Config {
        #[command(subcommand)]
        command: configuration::Command,
    },
    /// Describe this binary and its compiled features.
    Version {
        /// Print the structured product identity instead of a text line.
        #[arg(long)]
        json: bool,
    },
    /// List asynchronous questions waiting for a human decision.
    Approvals(approvals::ListArgs),
    /// Answer an agent's question and queue the answer for its next turn.
    Respond(approvals::RespondArgs),
    /// Serve external blocking-call tools over MCP stdio.
    Mcp,
    /// Inspect and maintain provider login profiles.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// Inspect and upgrade controller data while all state writers are stopped.
    State {
        #[command(subcommand)]
        command: StateCommand,
    },
    /// Compact diagnostic event history while the daemon is stopped.
    Compact {
        /// Prune events strictly older than this Unix timestamp (0..9223372036854775807).
        #[arg(long, value_parser = hive_input::clap::parsers::unsigned(
            crate::limits::COMPACT_BEFORE
        ))]
        before: u64,
        /// Retain at least this many recent events per agent (1..9223372036854775807).
        #[arg(long, default_value_t = DEFAULT_KEEP_EVENTS,
             value_parser = hive_input::clap::parsers::unsigned(crate::limits::COMPACT_KEEP))]
        keep_per_agent: u64,
        /// Preview eligible pruning without recovery, deletion, manifest writes or vacuum.
        #[arg(long)]
        dry_run: bool,
    },
    /// Read a bounded UTF-8 byte range from a large event payload.
    Output(requests::Output),
    /// Discover supported per-agent options and native configuration keys.
    Options {
        #[command(subcommand)]
        command: AgentOptionsCommand,
    },
    /// Discover provider models without inference using shared credential ownership.
    Models {
        #[arg(long, value_parser = hive_input::clap::parsers::config_name)]
        /// Configured subscription authentication profile.
        auth: String,
    },
    /// Verify native Claude hook steering with one small inference (daemon stopped).
    VerifySteering {
        #[arg(long, value_parser = hive_input::clap::parsers::config_name)]
        /// Configured Claude subscription authentication profile.
        auth: String,
        #[arg(long, value_parser = hive_input::clap::parsers::model)]
        /// Provider model token (1-128 ASCII bytes); validated against provider discovery.
        model: String,
        #[arg(long, value_parser = hive_input::clap::parsers::effort)]
        /// Reasoning effort supported by the selected model.
        effort: String,
    },
    /// Run and control the background controller.
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    /// Start an agent or branch from an existing conversation.
    Spawn(Box<spawn::Args>),
    /// List agents, or find quiet groups with empty mailboxes.
    Agents(requests::Agents),
    /// Atomically accept up to 64 spawn/send/terminate operations from a JSON array.
    Batch(batch::Batch),
    /// Inspect one agent's state and saved settings.
    Status(requests::Status),
    /// Read one event page, or follow an agent's activity for human observation.
    Read(read::Read),
    /// Snapshot an agent for live viewing, or follow its streamed output.
    Live(live::Live),
    /// Open a terminal chat view of one agent: history, streamed output and input.
    Attach(attach::Attach),
    #[doc = " Keep one zellij, tmux or Ghostty pane open per live agent, grouped by parent, \
        closed as agents end."]
    Panes(panes::Panes),
    #[doc = " One terminal for many agents: a tree sidebar, tabs and splits of chat views, an \
        attention queue and a spawn dialog."]
    Tui(tui::Tui),
    /// Read a page of an agent's incoming messages.
    Messages(requests::Messages),
    /// Queue, steer, redirect or schedule a message for an agent.
    Send(send::Send),
    /// Change an agent's login profile or fallback group.
    SetAuth(requests::SetAuth),
    /// Persist agent options for the next turn boundary.
    SetOptions(requests::SetOptions),
    /// Change or clear an agent's display name at once.
    SetName(requests::SetName),
    /// Mark an agent's task complete and retain its final result.
    Complete(requests::Complete),
    /// Deliberately share public review or human correction evidence.
    #[cfg(feature = "comb")]
    Evidence(collectors::Evidence),
    /// Inspect and administer retained Comb evidence.
    #[cfg(feature = "comb")]
    Collectors(collectors::Collectors),
    /// Stop an agent's execution while retaining its resumable conversation.
    Stop(requests::Control),
    /// Permanently cancel this agent and request asynchronous provider shutdown.
    Terminate(requests::Control),
    /// Delete an ended agent's `Hive` history and output; retain native provider history.
    Purge(requests::Purge),
    /// Purge safely released agents that permanently ended before a retention cutoff.
    Gc {
        /// Required retention age: a positive integer followed by s, m, h, d or w.
        #[arg(long, value_parser = duration::parse)]
        older_than: u64,
        /// Preview eligible agents and cleanup blockers without deleting anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Resume a stopped or failed agent using its saved conversation.
    Resume(requests::Control),
    /// Interrupt the active turn without terminating the agent.
    Interrupt(requests::Control),
    /// Wait for an agent outcome, one message reply or a joined batch.
    Wait(wait::Wait),
}

#[derive(Subcommand)]
enum StateCommand {
    /// Validate the stored format and show retained migration history and backups.
    Status,
    /// Apply registered released-format steps; development data is never migrated.
    Upgrade {
        /// Validate and list planned steps without changing the state directory.
        #[arg(long)]
        dry_run: bool,
    },
}
#[derive(Subcommand)]
enum DaemonCommand {
    /// Run the controller in the foreground.
    Run,
    /// Start the controller in the background.
    Start,
    /// Stop the controller, confirm cleanup, and start it with the selected build.
    Restart,
    /// Stop the running controller and its provider processes.
    Stop,
    /// Inspect controller readiness and capacity.
    Status,
}
#[derive(Subcommand)]
enum AuthCommand {
    /// List login groups with their mode, weights and per-member load.
    Groups,
    /// Show the newest 5-hour, weekly and credit usage each login reported.
    Usage {
        #[arg(value_parser = hive_input::clap::parsers::config_name)]
        /// Configured authentication profile name; every login when omitted.
        name: Option<String>,
    },
    /// List configured profiles without starting a provider or creating state.
    List,
    /// Inspect credentials; this does not run an inference turn.
    Status {
        #[arg(value_parser = hive_input::clap::parsers::config_name)]
        /// Configured authentication profile name.
        name: String,
    },
    /// Run the provider's native subscription login in your terminal.
    Login {
        #[arg(value_parser = hive_input::clap::parsers::config_name)]
        /// Configured authentication profile name.
        name: String,
        #[arg(long, help = "Use Codex browser login instead of device login")]
        browser: bool,
        #[arg(
            long,
            help = "Request a new native login for an existing subscription of the same method"
        )]
        reauthenticate: bool,
    },
    /// Reconcile saved process identities and report execution fences without rebooting.
    Recover {
        #[arg(value_parser = hive_input::clap::parsers::config_name)]
        /// Configured authentication profile name.
        name: String,
    },
    /// List active and retired credential generations and their audit metadata.
    Generations {
        #[arg(value_parser = hive_input::clap::parsers::config_name)]
        /// Configured authentication profile name.
        name: String,
    },
    /// Export retired controller history without opening its database or locks.
    HistoryExport {
        #[arg(long)]
        /// New directory for exported retired controller history.
        output: PathBuf,
    },
    /// Remove an unreferenced, resolved retired generation (daemon must be stopped).
    Prune {
        #[arg(value_parser = hive_input::clap::parsers::config_name)]
        /// Configured authentication profile name.
        name: String,
        #[arg(long, value_parser = hive_input::validate::uuid)]
        /// Retired credential generation UUID to prune.
        generation: uuid::Uuid,
    },
    /// Authenticate an isolated credential generation without changing old agents.
    Repair {
        #[arg(value_parser = hive_input::clap::parsers::config_name)]
        /// Configured authentication profile name.
        name: String,
        #[arg(long)]
        /// Fresh controller state directory for the repaired login.
        new_state_dir: Option<PathBuf>,
        #[arg(long, required = true)]
        /// Required confirmation to authenticate a new isolated generation.
        reauthenticate: bool,
        #[arg(long)]
        /// Use Codex browser login instead of device login.
        browser: bool,
    },
}

impl Command {
    fn request_id(&self) -> Option<&String> {
        match self {
            Self::Spawn(args) => args.request_id.as_ref(),
            Self::Respond(args) => args.request_id.as_ref(),
            Self::Batch(args) => args.request_id.as_ref(),
            Self::Send(args) => args.request_id.as_ref(),
            Self::SetAuth(args) => args.request_id.as_ref(),
            Self::SetOptions(args) => args.request_id.as_ref(),
            Self::SetName(args) => args.request_id.as_ref(),
            Self::Complete(args) => args.request_id.as_ref(),
            #[cfg(feature = "comb")]
            Self::Evidence(args) => args.request_id.as_ref(),
            #[cfg(feature = "comb")]
            Self::Collectors(args) => args.request_id.as_ref(),
            Self::Stop(args)
            | Self::Resume(args)
            | Self::Interrupt(args)
            | Self::Terminate(args) => args.request_id.as_ref(),
            Self::Purge(args) => args.request_id.as_ref(),
            _ => None,
        }
    }
}

struct CommandOutcome {
    value: serde_json::Value,
    success: bool,
    input: Option<remote::InputReply>,
}

impl CommandOutcome {
    fn ok(value: serde_json::Value) -> Self {
        Self {
            input: None,
            value,
            success: true,
        }
    }
    fn failed(value: serde_json::Value) -> Self {
        Self {
            input: None,
            value,
            success: false,
        }
    }
}
impl From<crate::calls::CallResult> for CommandOutcome {
    fn from(result: crate::calls::CallResult) -> Self {
        Self {
            input: None,
            success: crate::calls::successful(&result),
            value: crate::calls::value(result),
        }
    }
}
pub(crate) fn run(daemon: bool) {
    run_parsed(parsing::read(daemon));
}
#[cfg(test)]
pub(crate) fn run_fixture(args: &[OsString]) {
    run_parsed(parsing::read_args(args, false));
}

fn stalled_cursor(raw: &str) -> std::result::Result<api::StalledCursor, &'static str> {
    // Parser sources can include cursor values; retain only trusted diagnostics.
    serde_json::from_str(raw).map_err(|_| "a JSON object with idle_since and root_id")
}

#[derive(clap::Subcommand)]
enum AgentOptionsCommand {
    /// List supported keys, scopes, defaults, mappings and owned refusals.
    Catalog {
        /// Limit the catalog to codex or claude; omit to list both providers.
        #[arg(long, value_enum)]
        provider: Option<crate::config::Provider>,
        /// Filter recorded model-specific capabilities by a provider model token.
        #[arg(long, value_parser = hive_input::clap::parsers::model)]
        model: Option<String>,
    },
}
