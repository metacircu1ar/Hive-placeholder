use super::args::{self, CredentialCommand, RawCommand};
use std::path::PathBuf;

pub(super) enum Command {
    Local(LocalCommand),
    Remote(RemoteCommand),
}

pub(super) enum LocalCommand {
    Service(hive_setup_support::service::Command, hive_log::Config),
    Logs(super::logs::Logs),
    Setup(Box<comb::setup::Prepared>),
    SetupMaintenance(hive_setup_support::maintenance::Args),
    Version {
        json: bool,
    },
    Model(Box<args::ModelCommand>),
    Init {
        dir: Option<PathBuf>,
    },
    Serve {
        args: args::Serve,
        config: Option<comb::embedding::Config>,
        plan: Option<Box<comb::provision::ServePlan>>,
    },
    Mcp {
        connection: comb::service::auth::Connection,
    },
    Embed {
        args: args::Embed,
        config: comb::embedding::Config,
    },
    Rebuild,
    Upgrade {
        dry_run: bool,
    },
}

pub(super) enum RemoteCommand {
    Get {
        slug: String,
    },
    Put(args::Put),
    Append(args::Append),
    Capture(args::Capture),
    Promote(args::Promote),
    Consolidate(args::Consolidate),
    Decide(args::Decide),
    Rm(args::Rm),
    List(args::List),
    Search(args::Search),
    Links(args::LinkArgs),
    Backlinks(args::LinkArgs),
    Graph {
        slug: String,
        depth: usize,
    },
    Import {
        dir: PathBuf,
        prefix: Option<String>,
    },
    ImportStatus,
    ImportAbandon {
        batch_id: comb::values::ImportId,
    },
    Sync,
    Doctor,
    Stats,
    Credential(CredentialCommand),
}

impl TryFrom<RawCommand> for Command {
    type Error = super::input::Failure;
    fn try_from(command: RawCommand) -> Result<Self, Self::Error> {
        use LocalCommand as Local;
        use RemoteCommand as Remote;
        Ok(match command {
            RawCommand::Service { command, logging } => {
                Self::Local(Local::Service(command, logging.stored()?))
            }
            RawCommand::Logs(args) => Self::Local(Local::Logs(args)),
            RawCommand::SetupMaintenance(options) => Self::Local(Local::SetupMaintenance(options)),
            RawCommand::Setup(options) => Self::Local(Local::Setup(Box::new(
                super::input::setup::prepare(options)?,
            ))),
            RawCommand::Model { command } => Self::Local(Local::Model(Box::new(command))),
            RawCommand::Version { json } => Self::Local(Local::Version { json }),
            RawCommand::Init { dir } => Self::Local(Local::Init { dir }),
            RawCommand::Upgrade { dry_run } => Self::Local(Local::Upgrade { dry_run }),
            RawCommand::Serve(args) => {
                // Reject ambient logging overrides before managed entry or mutation.
                args.logging.backend()?;
                let config = args
                    .embedding_config
                    .as_deref()
                    .map(|path| {
                        super::input::Context::new("serve", "--embedding-config").embedding(path)
                    })
                    .transpose()?;
                Self::Local(Local::Serve {
                    args,
                    config,
                    plan: None,
                })
            }
            RawCommand::Mcp { credential } => Self::Local(Local::Mcp {
                connection: super::input::Context::new("mcp", "--credential")
                    .credential(&credential)?,
            }),
            RawCommand::Embed(args) => {
                let config =
                    super::input::Context::new("embed", "--config").embedding(&args.config)?;
                Self::Local(Local::Embed { args, config })
            }
            RawCommand::Sync { rebuild: true } => Self::Local(Local::Rebuild),
            RawCommand::Get { slug } => Self::Remote(Remote::Get { slug }),
            RawCommand::Put(args) => Self::Remote(Remote::Put(args)),
            RawCommand::Append(args) => Self::Remote(Remote::Append(args)),
            RawCommand::Capture(args) => Self::Remote(Remote::Capture(args)),
            RawCommand::Promote(args) => Self::Remote(Remote::Promote(args)),
            RawCommand::Consolidate(args) => Self::Remote(Remote::Consolidate(args)),
            RawCommand::Decide(args) => Self::Remote(Remote::Decide(args)),
            RawCommand::Rm(args) => Self::Remote(Remote::Rm(args)),
            RawCommand::List(args) => Self::Remote(Remote::List(args)),
            RawCommand::Search(args) => Self::Remote(Remote::Search(args)),
            RawCommand::Links(args) => Self::Remote(Remote::Links(args)),
            RawCommand::Backlinks(args) => Self::Remote(Remote::Backlinks(args)),
            RawCommand::Graph { slug, depth } => Self::Remote(Remote::Graph { slug, depth }),
            RawCommand::Import {
                dir,
                prefix,
                status,
                abandon,
            } => Self::Remote(import(dir, prefix, status, abandon)?),
            RawCommand::Sync { rebuild: false } => Self::Remote(Remote::Sync),
            RawCommand::Doctor => Self::Remote(Remote::Doctor),
            RawCommand::Stats => Self::Remote(Remote::Stats),
            RawCommand::Credential { command } => Self::Remote(Remote::Credential(command)),
        })
    }
}

impl RemoteCommand {
    pub(super) fn name(&self) -> &'static str {
        match self {
            Self::Get { .. } => "get",
            Self::Put(_) => "put",
            Self::Append(_) => "append",
            Self::Capture(_) => "capture",
            Self::Promote(_) => "promote",
            Self::Consolidate(_) => "consolidate",
            Self::Decide(_) => "decide",
            Self::Rm(_) => "rm",
            Self::List(_) => "list",
            Self::Search(_) => "search",
            Self::Links(_) => "links",
            Self::Backlinks(_) => "backlinks",
            Self::Graph { .. } => "graph",
            Self::Import { .. } | Self::ImportStatus | Self::ImportAbandon { .. } => "import",
            Self::Sync => "sync",
            Self::Doctor => "doctor",
            Self::Stats => "stats",
            Self::Credential(CredentialCommand::Create { .. }) => "credential create",
            Self::Credential(CredentialCommand::List) => "credential list",
            Self::Credential(CredentialCommand::Revoke { .. }) => "credential revoke",
        }
    }
}

fn import(
    dir: Option<PathBuf>,
    prefix: Option<String>,
    status: bool,
    abandon: Option<String>,
) -> super::input::Result<RemoteCommand> {
    Ok(match (dir, prefix, status, abandon) {
        (Some(dir), prefix, false, None) => RemoteCommand::Import { dir, prefix },
        (None, None, true, None) => RemoteCommand::ImportStatus,
        (None, None, false, Some(batch_id)) => RemoteCommand::ImportAbandon {
            batch_id: batch_id.try_into()?,
        },
        _ => return Err(comb::Error::invalid_import_directory().into()),
    })
}
