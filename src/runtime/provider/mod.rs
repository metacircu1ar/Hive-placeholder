//! Provider process ownership, bounded shards, and protocol transport.
//!
//! Binary identity, catalogs, native Claude, protocol and error namespaces are
//! crate-internal facades. Their implementation modules remain private; callers
//! use the explicit items exported at each protocol or service boundary.

mod acquire;
pub(crate) mod binary;
pub(crate) mod catalogs;
pub(super) mod claude;
mod cleanup;
pub(crate) mod error;
mod launch;
mod limits;
pub(super) mod live_tap;
mod owned;
mod pool;
pub(super) mod protocol;
mod queue;
mod registry;
mod rpc;
mod session;
mod shard;
pub(super) mod usage;

pub(super) use launch::{Backend, Launch, LaunchOwner};
pub(super) use owned::start_owned_rpc;
pub(super) use pool::{Pool, SessionRequest};
pub(super) use registry::RecoveredReservation;
pub(super) use rpc::{ModelLookup, Rpc, Shutdown, ShutdownMethod};
pub(super) use session::Session;
pub(super) use shard::Started;

#[cfg(test)]
pub(super) use shard::corpus_support::WorkerTransport;

#[cfg(test)]
pub(super) use rpc::test_support::RpcHarness;
