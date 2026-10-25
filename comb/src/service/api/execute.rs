//! Typed snapshot and administrative execution after admission.
use super::{
    Capabilities,
    actions::{AdminAction, ReadAction},
};
use crate::{
    Comb, Result,
    error::value,
    index::search::QueryVector,
    read::Snapshot,
    service::auth::{Authority, Registry},
};
use serde::Serialize;
use serde_json::Value;

pub(crate) trait ReadExecution {
    fn execute(
        self,
        snapshot: &Snapshot,
        query: QueryVector,
        runtime: Option<crate::embedding::RuntimeInfo>,
        authority: &Authority,
    ) -> Result<Value>;
}
impl ReadExecution for ReadAction<'_> {
    fn execute(
        self,
        snapshot: &Snapshot,
        query: QueryVector,
        runtime: Option<crate::embedding::RuntimeInfo>,
        authority: &Authority,
    ) -> Result<Value> {
        match self {
            Self::Capabilities => value(Capabilities {
                protocols: comb_protocol::compatibility::SERVICE_RANGE,
                reviewer: authority.reviewer,
            }),
            Self::Health => value(health(snapshot)?),
            Self::Get(slug) => value(snapshot.index.document(slug)?),
            Self::Search(request, mode) => value(snapshot.index.retrieve(request, mode, query)?),
            Self::List(request) => value(snapshot.index.list(request)?),
            Self::Links(slug, incoming, after, limit) => {
                value(snapshot.index.links_page(slug, incoming, after, limit)?)
            }
            Self::Graph(slug, depth) => value(snapshot.index.graph(slug, depth)?),
            Self::Stats => {
                let mut stats = snapshot.index.stats()?;
                stats.embedding_runtime = runtime;
                value(stats)
            }
        }
    }
}

pub(crate) struct PreparedAdmin<'a>(AdminAction<'a>);
pub(crate) trait AdminPreparation<'a> {
    fn prepare(self, authority: &Authority) -> Result<PreparedAdmin<'a>>;
}
impl<'a> AdminPreparation<'a> for AdminAction<'a> {
    fn prepare(self, authority: &Authority) -> Result<PreparedAdmin<'a>> {
        authority.permit(self.required())?;
        Ok(PreparedAdmin(self))
    }
}
#[derive(Serialize)]
struct Revoked<'a> {
    revoked: &'a crate::values::CredentialId,
}
impl PreparedAdmin<'_> {
    pub(crate) fn execute(self, comb: &mut Comb, registry: &Registry) -> Result<Value> {
        match self.0 {
            AdminAction::Doctor => value(comb.doctor()?),
            AdminAction::Sync => value(comb.sync()?),
            AdminAction::Import(directory, prefix) => value(comb.import(directory, prefix)?),
            AdminAction::ImportStatus => value(comb.import_status()?),
            AdminAction::ImportAbandon(batch_id) => value(comb.abandon_import(batch_id)?),
            AdminAction::CredentialCreate(grant) => value(registry.create_input(grant.clone())?),
            AdminAction::CredentialList => value(registry.list()?),
            AdminAction::CredentialRevoke(id) => {
                registry.revoke(id)?;
                value(Revoked { revoked: id })
            }
        }
    }
}

pub(in crate::service) fn health(
    snapshot: &Snapshot,
) -> Result<comb_protocol::service::api::Health> {
    use comb_protocol::service::api::{Health, ModelHealth};
    let coverage = snapshot
        .index
        .embedding_coverage(crate::search::Filter::default())?;
    let build = crate::build_info();
    Ok(Health {
        managed_service: hive_setup_support::managed::label().map(str::to_owned),
        product_version: build.product_version.into(),
        compiled_features: build
            .compiled_features
            .iter()
            .map(|s| (*s).into())
            .collect(),
        model: snapshot
            .index
            .active_space()?
            .map(|(lease, space)| ModelHealth {
                space_id: lease.fingerprint.to_string(),
                name: space.model,
                revision: space.revision,
                backend: space.backend,
                dimensions: space.dimensions,
                query_ready: false,
            }),
        total: coverage.total,
        embedded: coverage.embedded,
        pending: coverage.pending,
        rejected: coverage.rejected,
    })
}
