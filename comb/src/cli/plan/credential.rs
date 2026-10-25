//! Credential output is reserved before issuance; cleanup keeps the issuing authority.
use super::{AfterReply, Context, CredentialCommand, Grant, Operation, Plan};
use comb::{
    Comb, Error, Result,
    error::value,
    service::{
        self,
        auth::{Connection, Credential, CredentialOutput, with_cleanup},
    },
    values::CredentialId,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

pub(super) struct Output {
    path: PathBuf,
    prepared: Option<CredentialOutput>,
}
#[derive(Serialize)]
struct Saved {
    credential_id: CredentialId,
    file: PathBuf,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Revoked {
    revoked: CredentialId,
}

pub(super) fn plan(command: CredentialCommand) -> crate::cli::input::Result<Plan> {
    Ok(match command {
        CredentialCommand::Create {
            name,
            out,
            bridge,
            reviewer,
            reviewer_role,
        } => {
            let prepared = crate::cli::input::Context::new("credential create", "--out")
                .file(&out)
                .credential_output(&out)?;
            Plan {
                operation: Operation::CredentialCreate {
                    grant: if bridge {
                        Grant::Bridge {
                            name,
                            reviewer_roles: reviewer_role,
                        }
                    } else {
                        Grant::Client { name, reviewer }
                    },
                },
                embedding_config: None,
                after: AfterReply::SaveCredential(Box::new(Output {
                    path: out,
                    prepared: Some(prepared),
                })),
            }
        }
        CredentialCommand::List => Plan::print(Operation::CredentialList),
        CredentialCommand::Revoke { id } => {
            Plan::print(Operation::CredentialRevoke { id: id.try_into()? })
        }
    })
}
impl Output {
    pub(super) fn cancel(&mut self) -> Result<()> {
        self.prepared
            .take()
            .map_or(Ok(()), CredentialOutput::cancel)
    }
    pub(super) fn apply(
        &mut self,
        context: &Context,
        human: &Connection,
        runtime: &tokio::runtime::Runtime,
        reply: Value,
    ) -> Result<Value> {
        let credential: Credential = serde_json::from_value(reply)?;
        if credential.comb_id != human.credential.comb_id || credential.id.is_human() {
            return Err(Error::invalid_credential_identity());
        }
        let id = credential.id.clone();
        let output = self
            .prepared
            .take()
            .ok_or_else(Error::internal_serialization)?;
        let saved = output.publish(&Connection {
            format: hive_input::compatibility::Exact,
            socket: human.socket.clone(),
            credential,
        });
        publication_result(saved, &id, || revoke(context, human, runtime, &id))?;
        value(Saved {
            credential_id: id,
            file: self.path.clone(),
        })
    }
}
fn publication_result(
    saved: Result<()>,
    id: &CredentialId,
    revoke: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let Err(error) = saved else {
        return Ok(());
    };
    if error.file_committed {
        return Err(Error::credential_publication_uncertain(id, error));
    }
    let revoked = revoke();
    let primary = Error::credential_save_failed(id, error, revoked.is_ok());
    Err(with_cleanup(primary, revoked))
}
fn revoke(
    context: &Context,
    human: &Connection,
    runtime: &tokio::runtime::Runtime,
    id: &CredentialId,
) -> Result<()> {
    let operation = Operation::CredentialRevoke { id: id.clone() };
    let reply = if let Some(reply) = runtime.block_on(service::client::try_call(
        human,
        &operation,
        None,
        super::limits::REQUEST,
    ))? {
        reply
    } else {
        let mut comb = Comb::open(&context.comb)?;
        check_identity(&comb, human)?;
        service::offline(&mut comb, &operation, None)?
    };
    check_revoked(reply, id)
}
pub(super) fn check_identity(comb: &Comb, human: &Connection) -> Result<()> {
    if comb.identity() != human.credential.comb_id {
        return Err(Error::forbidden());
    }
    Ok(())
}
fn check_revoked(reply: Value, id: &CredentialId) -> Result<()> {
    if !reply.is_object() {
        return Err(Error::invalid_credential_identity());
    }
    let reply: Revoked = serde_json::from_value(reply)?;
    if reply.revoked != *id {
        return Err(Error::invalid_credential_identity());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
