//! Typed CLI arguments construct daemon requests without changing input-read order.
use super::{
    AgentsArgs, CompleteArgs, DEFAULT_AGENT_PAGE_ROWS, DEFAULT_MESSAGE_PAGE_ROWS,
    DEFAULT_OUTPUT_BYTES, Input, MessagesArgs, OutputArgs, PathBuf, Request, SetOptionsArgs,
    StatusArgs, TargetArgs, api, duration, stalled_cursor,
};

#[derive(clap::Args)]
pub(super) struct SetAuth {
    #[arg(value_parser = hive_input::clap::parsers::text(crate::limits::CONTROL_INPUT_BYTES))]
    /// Agent ID to operate on.
    target: String,
    #[arg(value_parser = hive_input::clap::parsers::config_name)]
    /// Configured login profile or fallback group.
    auth: String,
    #[arg(long, value_parser = hive_input::clap::parsers::request_id)]
    /// Stable retry key (1-128 bytes, no controls); reuse with identical input.
    pub(super) request_id: Option<String>,
}

impl SetAuth {
    pub(super) fn request(&self) -> Request {
        Request::SetAuth(api::SetAuthArgs {
            target: self.target.clone().into(),
            auth: self.auth.clone().into(),
        })
    }
}

#[derive(clap::Args)]
#[command(group(clap::ArgGroup::new("change").args(["name", "clear"]).required(true)))]
pub(super) struct SetName {
    #[arg(value_parser = hive_input::clap::parsers::text(crate::limits::CONTROL_INPUT_BYTES))]
    /// Agent ID to operate on.
    target: String,
    #[arg(value_parser = hive_input::clap::parsers::agent_name)]
    /// Display name (1-64 UTF-8 bytes, no controls or surrounding spaces); not unique.
    name: Option<String>,
    #[arg(long)]
    /// Remove the display name.
    clear: bool,
    #[arg(long, value_parser = hive_input::clap::parsers::request_id)]
    /// Stable retry key (1-128 bytes, no controls); reuse with identical input.
    pub(super) request_id: Option<String>,
}

impl SetName {
    pub(super) fn request(&self) -> Request {
        Request::SetName(api::SetNameArgs {
            target: self.target.clone().into(),
            name: self.name.clone().map_or(Input::Missing, Input::from),
            clear: if self.clear {
                true.into()
            } else {
                Input::Missing
            },
        })
    }
}

#[derive(clap::Args)]
#[command(group(clap::ArgGroup::new("change").args(["option",
     "native_config"]).required(true).multiple(true)))]
pub(super) struct SetOptions {
    #[arg(value_parser = hive_input::clap::parsers::text(crate::limits::CONTROL_INPUT_BYTES))]
    /// Agent whose desired options to change.
    target: String,
    #[command(flatten)]
    selection: super::agent_options::Options,
    #[arg(long, value_parser = hive_input::clap::parsers::request_id)]
    /// Stable retry key; reuse with identical input.
    pub(super) request_id: Option<String>,
}
impl SetOptions {
    pub(super) fn validate_options(&self) -> super::input::Result<()> {
        self.selection.values("set-options").map(|_| ())
    }

    pub(super) fn request(&self) -> super::input::Result<Request> {
        let (options, config) = self.selection.read("set-options")?;
        Ok(Request::SetOptions(SetOptionsArgs {
            target: self.target.clone().into(),
            options,
            config,
        }))
    }
}

#[derive(clap::Args)]
pub(super) struct Complete {
    #[arg(value_parser = hive_input::clap::parsers::text(crate::limits::CONTROL_INPUT_BYTES))]
    /// Agent ID to operate on.
    target: String,
    /// Final result as literal text; exactly one result source is required.
    #[arg(
        long,
        allow_hyphen_values = true,
        required_unless_present = "result_file",
        conflicts_with = "result_file"
    )]
    body: Option<String>,
    #[arg(long, required_unless_present = "body", conflicts_with = "body")]
    /// UTF-8 final result file (at most 65536 bytes).
    result_file: Option<PathBuf>,
    #[arg(long, value_parser = hive_input::clap::parsers::request_id)]
    /// Stable retry key (1-128 bytes, no controls); reuse with identical input.
    pub(super) request_id: Option<String>,
}

impl Complete {
    pub(super) fn run(
        &self,
        root: &std::path::Path,
        id: &str,
        request: Request,
    ) -> crate::error::Result<super::CommandOutcome> {
        let mut outcome = super::remote_response(root, id, request)?;
        if let Some(input) = outcome.input.take() {
            let input = input.file_source(
                "complete",
                "--body",
                "--result-file",
                self.result_file.as_deref(),
            )?;
            outcome.value = input.value();
            outcome.input = Some(input);
        }
        Ok(outcome)
    }

    pub(super) fn request(&self) -> super::input::Result<Request> {
        Ok(Request::Complete(CompleteArgs {
            target: self.target.clone().into(),
            body: super::input::text_source(
                "complete",
                "--body",
                "--result-file",
                self.body.as_deref(),
                self.result_file.as_deref(),
            )?
            .into(),
        }))
    }
}

#[derive(clap::Args)]
pub(super) struct Output {
    #[arg(value_parser = hive_input::clap::parsers::uuid)]
    /// Artifact UUID returned by a large-output event.
    id: String,
    #[arg(long, default_value_t = 0)]
    /// UTF-8 byte offset into the artifact (default 0).
    offset: u64,
    /// Bytes per page, 4–65536; four bytes allow a complete UTF-8 character.
    #[arg(long, default_value_t = DEFAULT_OUTPUT_BYTES,
         value_parser = clap::value_parser!(u64).range(
            crate::runtime::limits::MIN_OUTPUT_BYTES..=crate::runtime::limits::MAX_OUTPUT_BYTES
         ))]
    limit: u64,
}

impl Output {
    pub(super) fn request(&self) -> Request {
        Request::Output(OutputArgs {
            id: self.id.clone().into(),
            offset: self.offset.into(),
            limit: self.limit.into(),
        })
    }
}

#[derive(clap::Args)]
pub(super) struct Agents {
    #[arg(long)]
    /// Continue after this agent sequence cursor (default 0).
    after: Option<u64>,
    /// Maximum rows per page, 1–200 (default 200, or 100 with --stalled).
    #[arg(long, value_parser = clap::value_parser!(u64).range(
        crate::runtime::limits::MIN_PAGE_ROWS..=crate::runtime::limits::MAX_PAGE_ROWS
    ))]
    limit: Option<u64>,
    /// Report whole live groups with empty mailboxes and no activity for this duration.
    #[arg(long, value_parser = duration::parse, conflicts_with_all = ["after", "parent", "auth"])]
    stalled: Option<u64>,
    /// JSON cursor returned as `next_cursor` by a previous stalled page.
    #[arg(long, requires = "stalled", value_parser = stalled_cursor)]
    stalled_after: Option<api::StalledCursor>,
    #[arg(long, value_parser = hive_input::clap::parsers::text(crate::limits::CONTROL_INPUT_BYTES))]
    /// Filter direct children of this parent agent ID.
    parent: Option<String>,
    #[arg(long, value_parser = hive_input::clap::parsers::config_name)]
    /// Configured login profile or fallback group.
    auth: Option<String>,
}

impl Agents {
    pub(super) fn validate_cursor(&self) -> super::input::Result<()> {
        self.stalled_after.as_ref().map_or(Ok(()), |cursor| {
            cursor.validate().map_err(|error| match error {
                super::Error::Input { input, .. } => super::input::Context::new(
                    "agents",
                    "--stalled-after",
                )
                .invalid(input.code(), "Invalid stalled cursor.", input.expected()),
                error => error.into(),
            })
        })
    }

    pub(super) fn request(&self) -> Request {
        Request::Agents(AgentsArgs {
            after: self
                .after
                .or_else(|| self.stalled.is_none().then_some(0))
                .map_or(Input::Missing, Input::from),
            limit: self
                .limit
                .or_else(|| self.stalled.is_none().then_some(DEFAULT_AGENT_PAGE_ROWS))
                .map_or(Input::Missing, Input::from),
            parent: self.parent.clone().map_or(Input::Missing, Input::from),
            auth: self.auth.clone().map_or(Input::Missing, Input::from),
            stalled_seconds: self.stalled.map_or(Input::Missing, Input::from),
            stalled_after: self
                .stalled_after
                .clone()
                .map_or(Input::Missing, Input::from),
        })
    }
}

#[derive(clap::Args)]
pub(super) struct Status {
    #[arg(value_parser = hive_input::clap::parsers::text(crate::limits::CONTROL_INPUT_BYTES))]
    /// Agent ID to operate on.
    target: String,
    #[arg(long)]
    /// Include the agent instructions snapshot.
    instructions: bool,
}

impl Status {
    pub(super) fn request(&self) -> Request {
        Request::Status(StatusArgs {
            target: self.target.clone().into(),
            instructions: self.instructions.into(),
        })
    }
}

#[derive(clap::Args)]
pub(super) struct Messages {
    #[arg(value_parser = hive_input::clap::parsers::text(crate::limits::CONTROL_INPUT_BYTES))]
    /// Agent ID to operate on.
    target: String,
    #[arg(
        long,
        help = "Read older messages using the previous page's next_before cursor"
    )]
    before: Option<u64>,
    /// Maximum messages per page, 1–200.
    #[arg(long, default_value_t = DEFAULT_MESSAGE_PAGE_ROWS,
         value_parser = clap::value_parser!(u64).range(
            crate::runtime::limits::MIN_PAGE_ROWS..=crate::runtime::limits::MAX_PAGE_ROWS
         ))]
    limit: u64,
}

impl Messages {
    pub(super) fn request(&self) -> Request {
        Request::Messages(MessagesArgs {
            target: self.target.clone().into(),
            before: Input::nullable(self.before),
            limit: self.limit.into(),
        })
    }
}

#[derive(clap::Args)]
pub(super) struct Control {
    #[arg(value_parser = hive_input::clap::parsers::text(crate::limits::CONTROL_INPUT_BYTES))]
    /// Agent ID to operate on.
    target: String,
    #[arg(long, value_parser = hive_input::clap::parsers::request_id)]
    /// Stable retry key (1-128 bytes, no controls); reuse with identical input.
    pub(super) request_id: Option<String>,
}

impl Control {
    pub(super) fn request(&self) -> TargetArgs {
        TargetArgs {
            target: self.target.clone().into(),
        }
    }
}

#[derive(clap::Args)]
pub(super) struct Purge {
    #[arg(value_parser = hive_input::clap::parsers::text(crate::limits::CONTROL_INPUT_BYTES))]
    /// Agent ID to operate on.
    target: String,
    /// Include all descendants, including independent children.
    #[arg(long)]
    recursive: bool,
    /// Preview selection and blockers without deleting anything.
    #[arg(long)]
    dry_run: bool,
    #[arg(long, value_parser = hive_input::clap::parsers::request_id)]
    /// Stable retry key (1-128 bytes, no controls); reuse with identical input.
    pub(super) request_id: Option<String>,
}

impl Purge {
    pub(super) fn request(&self) -> Request {
        Request::Purge(api::PurgeArgs {
            target: self.target.clone().into(),
            recursive: self.recursive.into(),
            dry_run: self.dry_run.into(),
        })
    }
}
