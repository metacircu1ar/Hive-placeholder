//! Shared local service. Blocking file/model work never runs on an async reactor,
//! and embedding saturation cannot consume the independent read/write lanes.
mod api;
pub mod auth;
mod backfill;
pub mod client;
mod connection;
mod diagnostics;
mod dispatch;
mod framing;
mod input;
mod limits;
pub mod mcp;
mod observations;
mod options;
mod queries;
pub(crate) mod socket;
mod startup;
use crate::{
    Comb, Error, Result,
    embedding::{self, Embedder, Engine},
    index::Index,
    locks::{Health, Mutex as CheckedMutex},
    read::{Publication, Snapshot},
};
pub use api::Operation;
use api::{Reply, Request};
use auth::{Authority, Registry};
use dispatch::join_error;
pub use dispatch::offline;
use socket::Guard as SocketGuard;
pub use startup::{check_socket_output, socket_path};
#[cfg(test)]
use std::os::unix::fs::PermissionsExt;
#[cfg(test)]
use std::sync::{Mutex, atomic::Ordering};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicUsize},
    time::Duration,
};
use tokio::{
    io::BufReader,
    net::UnixListener,
    sync::{Notify, Semaphore},
};

pub use limits::{
    DEFAULT_GRAPH_DEPTH, DEFAULT_LINK_LIMIT, DEFAULT_VECTOR_CACHE_MIB, VECTOR_CACHE_MIB,
};

#[derive(Default)]
pub struct Options {
    pub socket: Option<PathBuf>,
    pub embeddings: Option<embedding::Config>,
    pub replace_embeddings: bool,
    pub retry_failed: bool,
    /// None uses the default budget when an engine is configured.
    pub vector_cache_mib: Option<usize>,
}
pub struct Server {
    state: Arc<State>,
    listener: std::os::unix::net::UnixListener,
    queries: tokio::sync::mpsc::Receiver<queries::Job>,
    _socket: SocketGuard,
}
/// Fully loaded server state; no listener is published until `bind` consumes it.
pub struct Prepared {
    state: Arc<State>,
    queries: tokio::sync::mpsc::Receiver<queries::Job>,
    lease: socket::Lease,
}
struct State {
    observations: observations::Counters,
    health: Arc<Health>,
    writer: CheckedMutex<Comb>,
    registry: Registry,
    index_path: PathBuf,
    publication: Arc<Publication>,
    resident: Option<Arc<crate::index::cache::Resident>>,
    model: Option<Model>,
    reads: Arc<Semaphore>,
    writes: Arc<Semaphore>,
    queries: queries::Queue,
    wake: Notify,
}
struct Model {
    runtime: Option<embedding::RuntimeInfo>,
    engine: CheckedMutex<Box<dyn Embedder>>,
    query_engine: Option<CheckedMutex<Box<dyn Embedder>>>,
    lease: embedding::Lease,
    pending_queries: Arc<AtomicUsize>,
}
#[cfg(test)]
mod tests;
