//! Human diagnostic viewing never opens the daemon database or creates state.
use super::{Cli, Error, Result};
use hive_log::reader::{Filter, Reader};
use std::io::{self, Write};

#[derive(Clone, Copy, clap::ValueEnum)]
enum Level {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}
impl Level {
    fn tracing(self) -> tracing::Level {
        match self {
            Self::Error => tracing::Level::ERROR,
            Self::Warn => tracing::Level::WARN,
            Self::Info => tracing::Level::INFO,
            Self::Debug => tracing::Level::DEBUG,
            Self::Trace => tracing::Level::TRACE,
        }
    }
}
#[derive(clap::Args)]
pub(super) struct Logs {
    /// Show the latest matching records (1..2000), inspecting at most 8 MiB.
    #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u64).range(1..=2000))]
    tail: u64,
    /// Minimum severity: warn includes warnings and errors.
    #[arg(long, value_enum)]
    level: Option<Level>,
    /// Match an exact static event name, such as agent.completed.
    #[arg(long, value_parser = event_name)]
    event: Option<String>,
    /// Match an exact agent identity.
    #[arg(long, value_parser = hive_input::clap::parsers::text(128))]
    agent: Option<String>,
    /// Continue printing complete records until Ctrl-C (humans only).
    #[arg(long)]
    follow: bool,
}
fn event_name(text: &str) -> std::result::Result<String, String> {
    if !text.is_empty()
        && text.len() <= 128
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
    {
        Ok(text.into())
    } else {
        Err("Expected an event name of 1..128 ASCII letters, digits, _, ., : or -".into())
    }
}
pub(super) fn run(cli: &Cli, args: &Logs) -> i32 {
    match view(
        cli,
        args,
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
    ) {
        Ok(()) => 0,
        Err(error) if error.io_kind() == Some(io::ErrorKind::BrokenPipe) => 0,
        Err(error) => super::parsing::emit_stderr(super::parsing::human(&error)),
    }
}
fn view(cli: &Cli, args: &Logs, out: &mut impl Write, notices: &mut impl Write) -> Result<()> {
    let root = cli.state_root()?;
    crate::state::check_path(&root, true)?;
    crate::state::check_path(&root.join("runtime"), true)?;
    let reader =
        Reader::open(&root.join("runtime/logs/hive-daemon.jsonl")).map_err(Error::state_io_from)?;
    let filter = Filter {
        level: args.level.map(Level::tracing),
        event: args.event.clone(),
        agent_id: args.agent.clone(),
    };
    let count = usize::try_from(args.tail)
        .map_err(|cause| Error::invalid_request("Invalid log tail size.").with_source(cause))?;
    reader
        .view(
            &hive_log::reader::View {
                tail: count,
                filter,
                follow: args.follow,
            },
            "Hive",
            out,
            notices,
        )
        .map_err(Error::state_io_from)
}
