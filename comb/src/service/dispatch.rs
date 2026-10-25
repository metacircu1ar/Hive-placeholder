//! Service request dispatch, background work and the offline adapter.
use super::api::{
    AdminPreparation as _, Capabilities, Class, Dispatch, MutationPreparation as _,
    OperationAdmission as _, ReadAction, ReadExecution as _,
};
use super::{
    Arc, Authority, Comb, Embedder, Engine, Error, Index, Operation, Registry, Request, Result,
    Snapshot, State, embedding,
};
use crate::search::QueryVector;
use tokio::sync::OwnedSemaphorePermit;

impl State {
    pub(super) fn snapshot(&self) -> Result<Snapshot> {
        self.health.check()?;
        Snapshot::pin(
            Index::reader(&self.index_path, self.resident.clone())?,
            &self.publication,
        )
    }
    pub(super) async fn handle(self: &Arc<Self>, request: Request) -> Result<serde_json::Value> {
        let method = super::observations::method(&request.operation);
        let started = std::time::Instant::now();
        let result = self.handle_inner(request).await;
        let code = result
            .as_ref()
            .err()
            .map_or("ok", |error| error.code.as_str());
        let partial = result.as_ref().is_err_and(|error| error.file_committed);
        tracing::debug!(name: "comb.request_completed",
             method = method,
             code = code,
             partial = partial,
             duration_ms = started.elapsed().as_millis());
        result
    }
    async fn handle_inner(self: &Arc<Self>, request: Request) -> Result<serde_json::Value> {
        if !comb_protocol::compatibility::SERVICE_RANGE.contains(request.protocol) {
            return Err(Error::incompatible_protocol());
        }
        self.health.check()?;
        let authority = self
            .registry
            .authorize(&request.credential, request.claim.as_ref())
            .inspect_err(|error| {
                tracing::debug!(name: "comb.auth_denied",
                     code = error.code.as_str(),
                     decision = "reject_request");
            })?;
        if matches!(request.operation, Operation::Capabilities) {
            self.health.check()?;
            return crate::error::value(Capabilities {
                protocols: comb_protocol::compatibility::SERVICE_RANGE,
                reviewer: authority.reviewer,
            });
        }
        request
            .operation
            .collector_admission(&authority)
            .inspect_err(|error| {
                tracing::debug!(name: "comb.auth_denied",
                 code = error.code.as_str(),
                 decision = "reject_collector_operation");
            })?;
        let query = self.prepare_query(&request.operation).await;
        let permit = self.admit(request.operation.class()).await?;
        let state = self.clone();
        let mutation = request.operation.is_mutation();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            state.execute(&request.operation, &authority, query)
        })
        .await
        .map_err(|cause| {
            // A panicked mutating writer may already have durable file/receipt
            // work. Poison fences access; recovery determines its final state.
            let committed = mutation && cause.is_panic();
            let mut error = self.health.error_or(join_error(cause));
            error.file_committed = committed;
            error
        })?
    }

    async fn prepare_query(self: &Arc<Self>, operation: &Operation) -> QueryVector {
        match operation.dispatch() {
            Dispatch::Read(action) => match action.query() {
                Some(text) => self.query(text).await.into(),
                None => QueryVector::NotRequested,
            },
            Dispatch::Mutation(_) | Dispatch::Admin(_) => QueryVector::NotRequested,
        }
    }

    async fn admit(&self, class: Class) -> Result<OwnedSemaphorePermit> {
        match class {
            Class::Read => self
                .reads
                .clone()
                .acquire_owned()
                .await
                .map_err(|cause| Error::busy("read").caused_by(cause)),
            Class::Write | Class::Admin => {
                self.writes.clone().try_acquire_owned().map_err(|cause| {
                    self.observations
                        .lost(super::observations::Loss::WriteAdmission, Some(&cause));
                    Error::busy("write").caused_by(cause)
                })
            }
        }
    }

    fn execute(
        &self,
        operation: &Operation,
        authority: &Authority,
        query: QueryVector,
    ) -> Result<serde_json::Value> {
        match operation.dispatch() {
            Dispatch::Read(ReadAction::Health) => {
                let mut health = super::api::health(&self.snapshot()?)?;
                if let Some(model) = &mut health.model {
                    model.query_ready = self.model.is_some();
                }
                crate::error::value(health)
            }
            Dispatch::Read(action) => action.execute(
                &self.snapshot()?,
                query,
                self.model.as_ref().and_then(|model| model.runtime.clone()),
                authority,
            ),
            Dispatch::Mutation(action) => {
                self.with_writer(|comb| action.prepare(authority)?.execute(comb))
            }
            Dispatch::Admin(action) => {
                self.with_writer(|comb| action.prepare(authority)?.execute(comb, &self.registry))
            }
        }
    }

    fn with_writer(
        &self,
        operation: impl FnOnce(&mut Comb) -> Result<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let mut comb = self.writer.lock()?;
        let result = operation(&mut comb);
        if let Err(error) = comb.ready() {
            tracing::warn!(name: "comb.publication_pending",
                 code = error.code.as_str(),
                 error = &error as &(dyn std::error::Error + 'static),
                 decision = "require_recovery");
            self.publication.fail();
        }
        self.wake.notify_one();
        result
    }
    pub(super) async fn query(self: &Arc<Self>, text: &str) -> Result<embedding::Query> {
        self.health.check()?;
        self.queries
            .query(
                text,
                self.model.as_ref().ok_or_else(Error::query_model_missing)?,
            )
            .await
    }
}
pub(super) fn join_error(cause: tokio::task::JoinError) -> Error {
    Error::worker_failed().caused_by(crate::error::Cause::worker(cause))
}

/// The human CLI's explicit offline path, used only when no request was sent.
pub fn offline(
    comb: &mut Comb,
    op: &Operation,
    engine: Option<Engine>,
) -> Result<serde_json::Value> {
    comb.health.check()?;
    if matches!(op, Operation::Capabilities) {
        return crate::error::value(Capabilities {
            protocols: comb_protocol::compatibility::SERVICE_RANGE,
            reviewer: true,
        });
    }
    if let Operation::Get { slug } = op {
        return crate::error::value(comb.get(slug)?);
    }
    match op.dispatch() {
        Dispatch::Read(action) => offline_read(comb, action, engine),
        Dispatch::Mutation(action) => {
            let (_registry, authority) = offline_authority(comb)?;
            action.prepare(&authority)?.execute(comb)
        }
        Dispatch::Admin(action) => {
            let (registry, authority) = offline_authority(comb)?;
            action.prepare(&authority)?.execute(comb, &registry)
        }
    }
}

fn offline_read(
    comb: &Comb,
    action: ReadAction<'_>,
    engine: Option<Engine>,
) -> Result<serde_json::Value> {
    let query = match action.query() {
        Some(text) => offline_query(comb, text, engine).into(),
        None => QueryVector::NotRequested,
    };
    let snapshot = Snapshot::pin(
        Index::reader(&comb.disk.index_path(), None)?,
        &comb.publication,
    )?;
    action.execute(
        &snapshot,
        query,
        None,
        &Authority::new(crate::Actor::Human, true)?,
    )
}

fn offline_query(comb: &Comb, text: &str, engine: Option<Engine>) -> Result<embedding::Query> {
    let mut engine = engine.ok_or_else(Error::query_model_missing)?;
    let lease = comb.index()?.embedding_lease(engine.space())?;
    embedding::run_query(&mut engine, lease, text)
}

fn offline_authority(comb: &Comb) -> Result<(Registry, Authority)> {
    let registry = Registry::open(&comb.disk, comb.health.clone())?;
    Ok((registry, Authority::new(crate::Actor::Human, true)?))
}
