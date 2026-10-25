//! Resolve command paths and retry identity before exhaustive local or remote routing.
use super::{
    Cli, Command, CommandOutcome, Config, DaemonCommand, PathBuf, Request, Result, State, api,
    approvals, auth_command, daemon_command, gc, models_command, remote_response, runtime,
};

/// Own the parsed command and its once-read payload before any startup work.
pub(super) struct Prepared {
    pub(super) cli: Cli,
    content: Option<Request>,
}
impl Prepared {
    pub(super) fn new(cli: Cli) -> super::input::Result<Self> {
        let content = match &cli.command {
            Command::Spawn(args) => Some(args.request()?),
            Command::Batch(args) => Some(args.request()?),
            Command::Send(args) => Some(args.request()?),
            Command::SetOptions(args) => Some(args.request()?),
            Command::Complete(args) => Some(args.request()?),
            Command::Approvals(args) => Some(args.request()?),
            Command::Respond(args) => Some(approvals::response_request(args)?),
            #[cfg(feature = "comb")]
            Command::Evidence(args) => Some(args.request()?),
            _ => None,
        };
        Ok(Self { cli, content })
    }
}

pub(super) fn execute(prepared: Prepared) -> Result<CommandOutcome> {
    if let Command::Config { command } = &prepared.cli.command {
        return super::configuration::run(&prepared.cli.config_path()?, command);
    }
    Invocation::new(prepared)?.run()
}

struct Invocation {
    content: Option<Request>,
    cli: Cli,
    root: PathBuf,
    request_id: String,
}

impl Invocation {
    fn new(Prepared { cli, content }: Prepared) -> Result<Self> {
        let root = cli.state_root()?;
        // Allocate one request identity before local or remote routing.
        let request_id = cli
            .command
            .request_id()
            .cloned()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        Ok(Self {
            content,
            cli,
            root,
            request_id,
        })
    }

    fn run(mut self) -> Result<CommandOutcome> {
        if let Some(request) = self.content.take() {
            return self.run_content(request);
        }
        let request = match &self.cli.command {
            Command::SetupMaintenance(options) => {
                return super::maintenance::run(&self.cli.config_path()?, &self.root, options);
            }
            Command::Setup(options) => {
                return super::setup::run(&self.cli, &self.cli.config_path()?, &self.root, options);
            }
            Command::Service { command } => return self.service(command),
            Command::Version { .. } => unreachable!("version output precedes ordinary commands"),
            Command::Config { .. } => unreachable!("config checks do not resolve state paths"),
            Command::Logs(_) | Command::Attach(_) | Command::Panes(_) | Command::Tui(_) => {
                unreachable!("views")
            }
            Command::Doctor => {
                return Ok(super::doctor::run(
                    &self.cli,
                    &self.cli.config_path()?,
                    &self.root,
                ));
            }
            Command::Mcp => unreachable!("stdio dispatch precedes ordinary JSON commands"),
            Command::Auth { command } => {
                return auth_command(command, self.root, &self.cli);
            }
            Command::State { command } => return self.state(command),
            Command::Approvals(_)
            | Command::Respond(_)
            | Command::Spawn(_)
            | Command::Batch(_)
            | Command::Send(_)
            | Command::SetOptions(_)
            | Command::Complete(_) => {
                unreachable!("content commands are prepared once before invocation")
            }
            Command::Gc {
                older_than,
                dry_run,
            } => return Ok(gc::run(&self.root, *older_than, *dry_run)),
            Command::Compact {
                before,
                keep_per_agent,
                dry_run,
            } => return self.compact(*before, *keep_per_agent, *dry_run),
            Command::VerifySteering {
                auth,
                model,
                effort,
            } => return self.verify_steering(auth, model, effort),
            Command::Options {
                command: super::AgentOptionsCommand::Catalog { provider, model },
            } => return catalog(*provider, model.as_deref()),
            Command::Models { auth } => return self.models(auth),
            Command::Daemon { command } => return self.daemon(command),
            Command::Wait(args) => return args.run(&self.root),
            Command::SetAuth(args) => args.request(),
            Command::SetName(args) => args.request(),
            #[cfg(feature = "comb")]
            Command::Evidence(_) => unreachable!("evidence is prepared before invocation"),
            #[cfg(feature = "comb")]
            Command::Collectors(args) => args.request(),
            Command::Output(args) => args.request(),
            Command::Agents(args) => args.request(),
            Command::Status(args) => args.request(),
            Command::Read(args) => args.request(),
            Command::Live(args) => args.request(),
            Command::Messages(args) => args.request(),
            Command::Stop(args) => Request::Stop(args.request()),
            Command::Terminate(args) => Request::Terminate(args.request()),
            Command::Purge(args) => args.request(),
            Command::Resume(args) => Request::Resume(args.request()),
            Command::Interrupt(args) => Request::Interrupt(args.request()),
        };
        remote_response(&self.root, &self.request_id, request)
    }

    fn run_content(&self, request: Request) -> Result<CommandOutcome> {
        match (&self.cli.command, request) {
            (Command::Send(args), Request::Send(input)) => {
                args.run(&self.root, &self.request_id, input)
            }
            (Command::Spawn(args), request) => args.run(&self.root, &self.request_id, request),
            (Command::Complete(args), request) => args.run(&self.root, &self.request_id, request),
            (Command::Batch(args), request) => args.run(&self.root, &self.request_id, request),
            (Command::Approvals(args), request) => args.run(&self.root, &self.request_id, request),
            (Command::Respond(args), request) => args.run(&self.root, &self.request_id, request),
            #[cfg(feature = "comb")]
            (Command::Evidence(args), request) => args.run(&self.root, &self.request_id, request),
            (_, request) => remote_response(&self.root, &self.request_id, request),
        }
    }

    fn service(&self, command: &hive_setup_support::service::Command) -> Result<CommandOutcome> {
        super::services::run(&self.cli, &self.cli.config_path()?, &self.root, command)
    }

    fn state(&self, command: &super::StateCommand) -> Result<CommandOutcome> {
        let dry_run = match command {
            super::StateCommand::Status => true,
            super::StateCommand::Upgrade { dry_run } => *dry_run,
        };
        let report = State::new(self.root.clone())?.upgrade(dry_run)?;
        Ok(CommandOutcome::ok(api::value(report)))
    }

    fn models(&self, auth: &str) -> Result<CommandOutcome> {
        models_command(
            &self.cli.config_path()?,
            self.root.clone(),
            auth,
            &self.binaries(),
        )
    }

    fn daemon(&self, command: &DaemonCommand) -> Result<CommandOutcome> {
        let config_path = self.cli.config_path()?;
        if !matches!(command, DaemonCommand::Run)
            && super::services::owned(&config_path, &self.root)?
        {
            if matches!(command, DaemonCommand::Restart) {
                return super::services::restart(&self.cli, &config_path, &self.root);
            }
            let service = match command {
                DaemonCommand::Start => hive_setup_support::service::Command::Start,
                DaemonCommand::Stop => hive_setup_support::service::Command::Stop,
                DaemonCommand::Status => hive_setup_support::service::Command::Status,
                DaemonCommand::Run | DaemonCommand::Restart => unreachable!(),
            };
            return super::services::run(&self.cli, &config_path, &self.root, &service);
        }
        daemon_command(
            command,
            config_path,
            self.root.clone(),
            self.binaries(),
            &self.request_id,
        )
    }

    fn binaries(&self) -> runtime::Binaries {
        runtime::Binaries {
            codex: self.cli.codex_bin.clone(),
            claude: self.cli.claude_bin.clone(),
        }
    }

    fn compact(&self, before: u64, keep_per_agent: u64, dry_run: bool) -> Result<CommandOutcome> {
        if dry_run {
            return Ok(CommandOutcome::ok(api::value(runtime::compact_preview(
                &self.root,
                before,
                keep_per_agent,
            )?)));
        }
        Ok(CommandOutcome::ok(api::value(runtime::compact(
            &self.root,
            before,
            keep_per_agent,
        )?)))
    }

    fn verify_steering(&self, auth: &str, model: &str, effort: &str) -> Result<CommandOutcome> {
        let config = Config::load(&self.cli.config_path()?)?;
        let profile = super::auth::selection::profile(&config, auth, "verify-steering", "--auth")?;
        super::auth::selection::steering(profile)?;
        runtime::validate_steering_selection(profile, model, effort)?;
        let binary = crate::process::executable(&self.cli.claude_bin)?;
        let value = runtime::verify(
            &config,
            &State::new(self.root.clone())?,
            auth,
            model,
            effort,
            &binary,
        )?;
        Ok(CommandOutcome::ok(value))
    }
}

/// The local options catalog; it needs no daemon.
fn catalog(
    provider: Option<crate::config::Provider>,
    model: Option<&str>,
) -> Result<CommandOutcome> {
    Ok(CommandOutcome::ok(crate::options::listing(
        provider, model,
    )?))
}
