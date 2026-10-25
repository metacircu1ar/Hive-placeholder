//! Reject explicitly supplied globals that the selected command does not use.
use super::relations::rejected;
use crate::cli::args::{Cli, RawCommand};
use hive_input::{Code, Rendered};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Global {
    Root,
    Socket,
    Run,
    ManagedService,
}

pub(super) fn validate(cli: &Cli, path: &str) -> Result<(), Rendered> {
    use Global::{ManagedService, Root, Run, Socket};
    let allowed = allowed(&cli.command);
    for (global, supplied, field, expected) in [
        (
            Root,
            cli.comb.is_some(),
            "--root",
            "a command other than version or mcp",
        ),
        (
            Socket,
            cli.socket.is_some(),
            "--socket",
            "setup, serve, mcp or a remote knowledge/credential command; sync without --rebuild",
        ),
        (
            Run,
            cli.run.is_some(),
            "--run",
            "put, append, capture, promote, consolidate or decide",
        ),
        (
            ManagedService,
            cli.managed_service.is_some(),
            "--managed-service",
            "serve",
        ),
    ] {
        if supplied && !allowed.contains(&global) {
            return Err(rejected(path, Code::ConflictingArguments, field, expected));
        }
    }
    Ok(())
}

fn allowed(command: &RawCommand) -> &'static [Global] {
    use Global::{ManagedService, Root, Run, Socket};
    // Exhaustive by design: a new command needs an explicit applicability decision.
    match command {
        RawCommand::Version { .. } => &[],
        RawCommand::Mcp { .. } => &[Socket],
        RawCommand::Serve(_) => &[Root, Socket, ManagedService],
        RawCommand::Model { .. }
        | RawCommand::SetupMaintenance(_)
        | RawCommand::Service { .. }
        | RawCommand::Logs(_)
        | RawCommand::Init { .. }
        | RawCommand::Upgrade { .. }
        | RawCommand::Embed(_)
        | RawCommand::Sync { rebuild: true } => &[Root],
        RawCommand::Put(_)
        | RawCommand::Append(_)
        | RawCommand::Capture(_)
        | RawCommand::Promote(_)
        | RawCommand::Consolidate(_)
        | RawCommand::Decide(_) => &[Root, Socket, Run],
        RawCommand::Setup(_)
        | RawCommand::Get { .. }
        | RawCommand::Rm(_)
        | RawCommand::List(_)
        | RawCommand::Search(_)
        | RawCommand::Links(_)
        | RawCommand::Backlinks(_)
        | RawCommand::Graph { .. }
        | RawCommand::Import { .. }
        | RawCommand::Sync { rebuild: false }
        | RawCommand::Doctor
        | RawCommand::Stats
        | RawCommand::Credential { .. } => &[Root, Socket],
    }
}
