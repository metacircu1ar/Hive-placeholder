use super::{Comb, Details, Path, Plan, Result, invalid, service};
use crate::service::{
    Operation,
    auth::{self, Connection, ConnectionExt as _},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Proof {
    digest: String,
    pub(super) revoked: bool,
}
pub(super) fn inspect(root: &Path, path: &Path) -> Result<Option<Proof>> {
    let Some(bytes) = hive_setup_support::files::read(path)? else {
        return Ok(None);
    };
    let connection = Connection::read(path)?;
    auth::inspect_setup_bridge(root, &connection, false).map_err(|e| {
        invalid(format!(
            "Retain bridge {}: cannot prove the exact setup grant. Inspect `comb --root \
        '{}' credential revoke {}`",
            path.display(),
            root.display(),
            connection.credential.id
        ))
        .caused_by(e)
    })?;
    Ok(Some(Proof {
        digest: format!("{:x}", Sha256::digest(bytes)),
        revoked: false,
    }))
}
pub(super) fn revoke(
    plan: &mut Plan<Details>,
    receipt: &Path,
    comb: Option<&mut Comb>,
) -> Result<()> {
    let Some(path) = &plan.scope.bridge else {
        return Ok(());
    };
    let root = plan
        .scope
        .root
        .as_ref()
        .ok_or_else(|| invalid("Missing bridge root."))?;
    let action = plan
        .removals
        .first()
        .ok_or_else(|| invalid("Missing bridge action."))?;
    if action.object.path != *path {
        return Err(invalid("Bridge action scope mismatch."));
    }
    if action.done {
        plan.remove(0, receipt)?;
        return Ok(());
    }
    let Some(proof) = &mut plan.details.bridge else {
        action.object.check()?;
        plan.remove(0, receipt)?;
        return Ok(());
    };
    let secret_path = if hive_setup_support::files::optional_metadata(path)?.is_some() {
        path
    } else {
        &action.quarantine
    };
    if let Some(bytes) = hive_setup_support::files::read(secret_path)?
        && format!("{:x}", Sha256::digest(bytes)) != proof.digest
    {
        return Err(invalid(
            "Bridge credential changed after revocation; retaining the new secret.",
        ));
    }
    if !proof.revoked {
        action.object.check()?;
        let bytes = hive_setup_support::files::read(path)?
            .ok_or_else(|| invalid("Admitted bridge disappeared before revocation."))?;
        if format!("{:x}", Sha256::digest(bytes)) != proof.digest {
            return Err(invalid("Bridge credential changed since admission."));
        }
        let connection = Connection::read(path)?;
        if auth::inspect_setup_bridge(root, &connection, true)? {
            let operation = Operation::CredentialRevoke {
                id: connection.credential.id.clone(),
            };
            let result = if let Some(comb) = comb {
                service::offline(comb, &operation, None).map(|_| ())
            } else {
                revoke_online_or_offline(root, &connection, &operation)
            };
            result.map_err(|e| {
                invalid(format!(
                    "Bridge retained at {}. Revoke the exact credential with `comb --root '{}' \
                credential revoke {}` before retrying purge",
                    path.display(),
                    root.display(),
                    connection.credential.id
                ))
                .caused_by(e)
            })?;
        }
        proof.revoked = true;
        plan.save(receipt)?;
    }
    plan.remove(0, receipt)?;
    Ok(())
}
fn revoke_online_or_offline(root: &Path, bridge: &Connection, operation: &Operation) -> Result<()> {
    let connection = Connection::human(root, bridge.socket.clone())?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let reply = runtime.block_on(service::client::try_call(
        &connection,
        operation,
        None,
        REVOCATION_TIMEOUT,
    ))?;
    if reply.is_none() {
        service::offline(&mut Comb::open(root)?, operation, None)?;
    }
    Ok(())
}

const REVOCATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
