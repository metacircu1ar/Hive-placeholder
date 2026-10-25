//! Command-line argument declarations.
use clap::{Args, Parser, Subcommand};
use comb::{search::Mode, service};
use std::path::PathBuf;

pub(super) use super::models::ModelCommand;
use super::parsing::values;
use comb_protocol::{limits, search::limits as search_limits, values::limits as value_limits};
use hive_input::clap::parsers;

const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (http)");

#[derive(Parser)]
#[command(version = VERSION, about = "Shared local knowledge for people and agents")]
pub(crate) struct Cli {
    #[arg(long, global = true, hide = true)]
    pub(super) managed_service: Option<PathBuf>,
    #[arg(long = "root", global = true)]
    pub(super) comb: Option<PathBuf>,
    #[arg(long, global = true)]
    pub(super) socket: Option<PathBuf>,
    #[arg(long, global = true, value_parser = parsers::run_name(value_limits::FIELD_BYTES))]
    pub(super) run: Option<String>,
    #[command(subcommand)]
    pub(super) command: RawCommand,
}
impl Cli {
    pub(crate) fn is_mcp(&self) -> bool {
        matches!(self.command, RawCommand::Mcp { .. })
    }
}

#[derive(Subcommand)]
pub(super) enum RawCommand {
    /// Install and control this root's user login service.
    Service {
        #[command(subcommand)]
        command: hive_setup_support::service::Command,
        #[command(flatten)]
        logging: super::logging::Settings,
    },
    /// Prepare a knowledge root and optional `Hive` bridge without hand-written configuration.
    Setup(comb::setup::Options),
    #[command(hide = true)]
    SetupMaintenance(hive_setup_support::maintenance::Args),
    /// Describe the installed binary and compiled embedding backends.
    Version {
        #[arg(long)]
        json: bool,
    },
    Model {
        #[command(subcommand)]
        command: ModelCommand,
    },
    Serve(Serve),
    /// Tail and filter structured diagnostics without opening knowledge state.
    Logs(super::logs::Logs),
    Mcp {
        #[arg(long)]
        credential: PathBuf,
    },
    Credential {
        #[command(subcommand)]
        command: CredentialCommand,
    },
    Init {
        dir: Option<PathBuf>,
    },
    /// Upgrade a stopped root using registered released-format steps.
    Upgrade {
        /// Validate and list steps without changing the knowledge root.
        #[arg(long)]
        dry_run: bool,
    },
    Get {
        #[arg(value_parser = values::slug)]
        slug: String,
    },
    Put(Put),
    Append(Append),
    Capture(Capture),
    Promote(Promote),
    Consolidate(Consolidate),
    Decide(Decide),
    Rm(Rm),
    List(List),
    Search(Search),
    Embed(Embed),
    Links(LinkArgs),
    Backlinks(LinkArgs),
    Graph {
        #[arg(value_parser = values::slug)]
        slug: String,
        #[arg(long, default_value_t = service::DEFAULT_GRAPH_DEPTH,
             value_parser = parsers::count(0..=limits::GRAPH_DEPTH_MAX))]
        depth: usize,
    },
    Import {
        #[arg(required_unless_present_any = ["status",
             "abandon"],
             conflicts_with_all = ["status",
             "abandon"])]
        dir: Option<PathBuf>,
        #[arg(long, requires = "dir",
             conflicts_with_all = ["status",
             "abandon"],
             value_parser = values::slug)]
        prefix: Option<String>,
        #[arg(long, conflicts_with = "abandon")]
        status: bool,
        #[arg(long, value_parser = values::import_id)]
        abandon: Option<String>,
    },
    Sync {
        #[arg(long)]
        rebuild: bool,
    },
    Doctor,
    Stats,
}
#[derive(Subcommand)]
pub(super) enum CredentialCommand {
    Create {
        #[arg(value_parser = values::name)]
        name: String,
        #[arg(long)]
        out: PathBuf,
        #[arg(long, conflicts_with = "reviewer")]
        bridge: bool,
        #[arg(long)]
        reviewer: bool,
        #[arg(long, requires = "bridge", value_parser = values::name)]
        reviewer_role: Vec<String>,
    },
    List,
    Revoke {
        #[arg(value_parser = parsers::uuid)]
        id: String,
    },
}

#[derive(Args)]
pub(super) struct RequestIdArg {
    #[arg(long, value_parser = parsers::request_id)]
    pub(super) request_id: Option<String>,
}

#[derive(Args)]
pub(super) struct FilterArgs {
    #[arg(long = "type", value_parser = values::kind)]
    pub(super) kind: Option<String>,
    #[arg(long, value_parser = values::tag)]
    pub(super) tag: Option<String>,
    #[arg(long, value_parser = values::prefix)]
    pub(super) prefix: Option<String>,
}

#[derive(Args)]
pub(super) struct LinkArgs {
    #[arg(value_parser = values::slug)]
    pub(super) slug: String,
    #[arg(long, value_parser = values::slug)]
    pub(super) after: Option<String>,
    #[arg(long, default_value_t = service::DEFAULT_LINK_LIMIT,
         value_parser = parsers::count(limits::LINK_LIMIT_MIN..=limits::LINK_LIMIT_MAX))]
    pub(super) limit: usize,
}

#[derive(Args)]
pub(super) struct Serve {
    #[command(flatten)]
    pub(super) logging: super::logging::Settings,
    #[arg(long)]
    pub(super) embedding_config: Option<PathBuf>,
    #[arg(long)]
    pub(super) replace_embeddings: bool,
    #[arg(long)]
    pub(super) retry_failed: bool,
    #[arg(long, value_parser = parsers::count(service::VECTOR_CACHE_MIB))]
    pub(super) vector_cache_mib: Option<usize>,
}

#[derive(Args)]
pub(super) struct Put {
    #[arg(value_parser = values::slug)]
    pub(super) slug: String,
    #[arg(long)]
    pub(super) file: Option<PathBuf>,
    #[arg(long, value_parser = values::hash)]
    pub(super) expected_hash: Option<String>,
    #[command(flatten)]
    pub(super) request: RequestIdArg,
}

#[derive(Args)]
pub(super) struct Append {
    #[arg(value_parser = values::slug)]
    pub(super) slug: String,
    pub(super) text: String,
    #[arg(long)]
    pub(super) source: String,
    #[arg(long, requires = "expected_hash")]
    pub(super) summary_file: Option<PathBuf>,
    #[arg(long, value_parser = values::hash)]
    pub(super) expected_hash: Option<String>,
    #[command(flatten)]
    pub(super) request: RequestIdArg,
}

#[derive(Args)]
pub(super) struct Capture {
    pub(super) text: String,
    #[command(flatten)]
    pub(super) request: RequestIdArg,
}

#[derive(Args)]
pub(super) struct Promote {
    #[arg(value_parser = values::slug)]
    pub(super) slug: String,
    #[arg(long, value_parser = values::hash)]
    pub(super) expected_hash: String,
    #[command(flatten)]
    pub(super) request: RequestIdArg,
}

#[derive(Args)]
pub(super) struct Consolidate {
    #[arg(value_parser = values::slug)]
    pub(super) note: String,
    #[arg(value_parser = values::slug)]
    pub(super) target: String,
    #[arg(long, value_parser = values::hash)]
    pub(super) note_hash: String,
    #[arg(long, value_parser = values::hash)]
    pub(super) target_hash: String,
    #[arg(long)]
    pub(super) summary_file: PathBuf,
    /// Explicitly copy the whole note again after inspecting rewritten history.
    #[arg(long)]
    pub(super) recopy: bool,
    #[command(flatten)]
    pub(super) request: RequestIdArg,
}

#[derive(Args)]
pub(super) struct Decide {
    #[arg(value_parser = values::slug)]
    pub(super) slug: String,
    #[arg(value_enum)]
    pub(super) decision: comb::page::Decision,
    #[arg(long)]
    pub(super) reason: String,
    #[arg(long, value_parser = values::hash)]
    pub(super) expected_hash: String,
    #[command(flatten)]
    pub(super) request: RequestIdArg,
}

#[derive(Args)]
pub(super) struct Rm {
    #[arg(value_parser = values::slug)]
    pub(super) slug: String,
    #[arg(long, value_parser = values::hash)]
    pub(super) expected_hash: String,
    #[command(flatten)]
    pub(super) request: RequestIdArg,
}

#[derive(Args)]
pub(super) struct List {
    #[command(flatten)]
    pub(super) filters: FilterArgs,
    #[arg(long)]
    pub(super) unverified: bool,
    #[arg(long, value_parser = values::slug)]
    pub(super) after: Option<String>,
    #[arg(long, value_parser = parsers::count(limits::LIST_LIMIT_MIN..=limits::LIST_LIMIT_MAX))]
    pub(super) limit: Option<usize>,
}

#[derive(Args)]
pub(super) struct Search {
    #[arg(value_parser = values::query)]
    pub(super) query: String,
    #[arg(long, value_enum, default_value_t = Mode::Hybrid)]
    pub(super) mode: Mode,
    #[arg(long)]
    pub(super) embedding_config: Option<PathBuf>,
    #[arg(long, default_value_t = comb::search::DEFAULT_SEARCH_LIMIT,
         value_parser = parsers::count(
            search_limits::SEARCH_LIMIT_MIN..=search_limits::SEARCH_LIMIT_MAX
         ))]
    pub(super) limit: usize,
    #[command(flatten)]
    pub(super) filters: FilterArgs,
    #[arg(long, value_parser = values::timestamp)]
    pub(super) after: Option<String>,
    #[arg(long, value_parser = values::timestamp)]
    pub(super) before: Option<String>,
}

#[derive(Args)]
pub(super) struct Embed {
    #[arg(long)]
    pub(super) config: PathBuf,
    #[arg(long)]
    pub(super) replace: bool,
    #[arg(long)]
    pub(super) retry_failed: bool,
}

#[cfg(test)]
mod tests;
