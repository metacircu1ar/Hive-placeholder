//! Human-only remedies selected from the actual command and native outcome.
use crate::cli::{catalog, command::LocalCommand};
use comb::{Error, Result};

#[derive(Clone, Copy)]
pub(in crate::cli) enum Exclusive {
    Init,
    Embed,
    Rebuild,
}
impl Exclusive {
    pub(in crate::cli) fn for_command(command: &LocalCommand) -> Option<Self> {
        match command {
            LocalCommand::Init { .. } => Some(Self::Init),
            LocalCommand::Embed { .. } => Some(Self::Embed),
            LocalCommand::Rebuild => Some(Self::Rebuild),
            _ => None,
        }
    }
    fn command(self) -> &'static str {
        match self {
            Self::Init => "init",
            Self::Embed => "embed",
            Self::Rebuild => "sync",
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum Hint {
    Exclusive(Exclusive),
    QueryModel,
}
impl Hint {
    pub(super) fn query(error: &Error, command: &str) -> Option<Self> {
        let missing = Error::query_model_missing();
        (command == "search" && error.code == missing.code && error.message == missing.message)
            .then_some(Self::QueryModel)
    }
    pub(super) fn render(self, error: &Error) -> Result<String> {
        let mut display = error.clone();
        let (command, remedy) = match self {
            Self::Exclusive(operation) => {
                display.message =
                    "This local command requires exclusive access to the knowledge root.".into();
                (
                    operation.command(),
                    "Stop the managed service with `comb --root ROOT service stop`, replacing \
                        ROOT with the same root used by this command. Otherwise stop the \
                        process that owns the root, then retry.",
                )
            }
            Self::QueryModel => (
                "search",
                "For offline semantic search, provide --embedding-config FILE. With a \
                    running service, configure its embedding model instead. Use --mode text \
                    for literal search without a model.",
            ),
        };
        let diagnostic = display.diagnostic().unwrap_or_else(|| display.to_string());
        let examples = catalog::examples(command).ok_or_else(Error::internal_serialization)?;
        Ok(format!("{diagnostic}\n{remedy}\n{}\n", examples.help()))
    }
}

pub(super) fn socket_directory(error: &Error) -> Option<String> {
    let expected = Error::unsafe_socket_parent();
    (error.code == expected.code && error.message == expected.message)
        .then(|| error.diagnostic())
        .flatten()
}

#[cfg(test)]
mod tests;
