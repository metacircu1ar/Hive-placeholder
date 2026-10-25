//! Convert caller input before connection setup; attribute only execution failures.
use comb::service::auth::ConnectionExt as _;
mod credential;
mod mutation;
mod query;

use super::{Context, args::CredentialCommand, command::RemoteCommand, limits, runtime};
use comb::{
    Comb, Error, Result,
    embedding::{Config, Engine},
    service::{
        self, Operation,
        auth::{Connection, Grant},
    },
};
use serde_json::Value;
#[cfg(test)]
use std::path::PathBuf;

pub(super) struct Plan {
    operation: Operation,
    embedding_config: Option<Config>,
    after: AfterReply,
}

enum AfterReply {
    Print,
    SaveCredential(Box<credential::Output>),
}

impl RemoteCommand {
    pub(super) fn plan(self, context: &Context) -> super::input::Result<Plan> {
        Ok(match self {
            Self::Get { slug } => Plan::print(Operation::Get {
                slug: slug.try_into()?,
            }),
            Self::Put(args) => Plan::print(args.operation(context)?),
            Self::Append(args) => Plan::print(args.operation(context)?),
            Self::Capture(args) => Plan::print(args.operation(context)?),
            Self::Promote(args) => Plan::print(args.operation(context)?),
            Self::Consolidate(args) => Plan::print(args.operation(context)?),
            Self::Decide(args) => Plan::print(args.operation(context)?),
            Self::Rm(args) => Plan::print(args.operation()?),
            Self::List(args) => Plan::print(args.operation()),
            Self::Search(args) => args.plan()?,
            Self::Links(args) => Plan::print(args.operation(false)?),
            Self::Backlinks(args) => Plan::print(args.operation(true)?),
            Self::Graph { slug, depth } => Plan::print(Operation::Graph {
                slug: slug.try_into()?,
                depth,
            }),
            Self::Import { dir, prefix } => Plan::print(Operation::Import {
                directory: super::input::Context::new("import", "DIR")
                    .file(&dir)
                    .directory()?,
                prefix: prefix.map(TryInto::try_into).transpose()?,
            }),
            Self::ImportStatus => Plan::print(Operation::ImportStatus),
            Self::ImportAbandon { batch_id } => Plan::print(Operation::ImportAbandon { batch_id }),
            Self::Sync => Plan::print(Operation::Sync),
            Self::Doctor => Plan::print(Operation::Doctor),
            Self::Stats => Plan::print(Operation::Stats),
            Self::Credential(command) => credential::plan(command)?,
        })
    }
}

impl Plan {
    pub(super) fn prepare(
        mut self,
        prepare: impl FnOnce() -> super::input::Result<()>,
    ) -> super::input::Result<Self> {
        if let Err(error) = prepare() {
            return Err(error.with_cleanup(self.cancel()));
        }
        Ok(self)
    }

    fn cancel(&mut self) -> Result<()> {
        match &mut self.after {
            AfterReply::Print => Ok(()),
            AfterReply::SaveCredential(output) => output.cancel(),
        }
    }

    fn print(operation: Operation) -> Self {
        Self {
            operation,
            embedding_config: None,
            after: AfterReply::Print,
        }
    }

    pub(super) fn execute(mut self, context: &Context) -> Result<Value> {
        let result = self.execute_inner(context);
        let cleanup = self.cancel();
        let result = match result {
            Ok(value) => cleanup.map(|()| value),
            Err(error) => Err(service::auth::with_cleanup(error, cleanup)),
        };
        result.map_err(|mut error| {
            if let Some(id) = self.operation.request_id() {
                error.request_id = Some(id.to_owned());
            }
            error
        })
    }

    fn execute_inner(&mut self, context: &Context) -> Result<Value> {
        let engine = self.embedding_config.take().map(Engine::load).transpose()?;
        let stored = comb::provision::settings(&context.comb)?;
        let socket = service::socket_path(
            &context.comb,
            context
                .socket
                .as_deref()
                .or_else(|| stored.as_ref().map(|s| s.socket.as_path())),
        )?;
        let connection = Connection::human(&context.comb, socket.clone())?;
        let rt = runtime()?;
        if engine.is_some()
            && rt
                .block_on(service::client::try_call(
                    &connection,
                    &Operation::Capabilities,
                    None,
                    limits::CAPABILITIES,
                ))?
                .is_some()
        {
            return Err(Error::server_embedding_config());
        }
        let result = if let Some(reply) = rt.block_on(service::client::try_call(
            &connection,
            &self.operation,
            None,
            limits::REQUEST,
        ))? {
            reply
        } else {
            tracing::debug!(name: "comb.offline_fallback", decision = "acquire_exclusive_writer");
            let mut comb = Comb::open(&context.comb)?;
            credential::check_identity(&comb, &connection)?;
            service::offline(&mut comb, &self.operation, engine)?
        };
        match &mut self.after {
            AfterReply::Print => Ok(result),
            AfterReply::SaveCredential(output) => output.apply(context, &connection, &rt, result),
        }
    }
}

#[cfg(test)]
mod tests;
