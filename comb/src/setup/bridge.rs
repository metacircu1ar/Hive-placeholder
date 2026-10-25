use super::{Comb, Path, Result, files, invalid, plan::Plan};
use crate::service::{
    self, Operation,
    auth::{self, Connection, ConnectionExt as _, Credential, Grant},
};

pub(super) fn inspect(plan: &Plan) -> Result<bool> {
    let Some(path) = &plan.bridge_file else {
        return Ok(false);
    };
    hive_setup_support::names::check_output_name(path)?;
    if !crate::disk::exists(path)? {
        return Ok(false);
    }
    files::owned(path, false)?;
    if plan.initialize {
        return Err(invalid(
            "Existing bridge belongs to another root; choose --bridge-file or inspect \
                it before setup.",
        ));
    }
    let connection = Connection::read(path)?;
    if connection.socket != plan.socket {
        return Err(invalid(
            "Existing bridge has another socket; it will not be overwritten.",
        ));
    }
    auth::inspect_bridge(&plan.root, &connection, &plan.reviewer_roles).map_err(|e| {
        invalid(
            "Existing bridge is foreign, revoked, broken, or has different reviewer \
            roles; it will not be overwritten.",
        )
        .caused_by(e)
    })?;
    Ok(true)
}
pub(super) fn apply(comb: &mut Comb, plan: &Plan) -> Result<bool> {
    let Some(path) = &plan.bridge_file else {
        return Ok(false);
    };
    if inspect(plan)? {
        return Ok(false);
    }
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Bridge file needs a parent directory."))?;
    files::directory(parent)?;
    let credential: Credential = serde_json::from_value(service::offline(
        comb,
        &Operation::CredentialCreate {
            grant: Grant::Bridge {
                name: "hive-setup".into(),
                reviewer_roles: plan.reviewer_roles.clone(),
            },
        },
        None,
    )?)?;
    publish(
        path,
        Connection {
            format: hive_input::compatibility::Exact,
            socket: plan.socket.clone(),
            credential,
        },
    )?;
    Ok(true)
}
fn publish(path: &Path, connection: Connection) -> Result<()> {
    let id = connection.credential.id.clone();
    connection.write(path).map_err(|e| {
        invalid(format!(
            "Bridge {id} was created but saving {} failed. Inspect that file and the \
        registry; use credential revoke {id} before retrying if no valid file was \
        saved.",
            path.display()
        ))
        .committed()
        .caused_by(e)
    })
}
