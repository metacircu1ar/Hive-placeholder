//! Independent service bounds; equal values need not represent the same policy.
use std::time::Duration;

pub const DEFAULT_VECTOR_CACHE_MIB: usize = 512;
pub const VECTOR_CACHE_MIB: std::ops::RangeInclusive<usize> = 1..=8192;
pub(super) const FRAME_SLOTS: usize = 128;
pub(super) const READ_SLOTS: usize = 8;
pub(super) const WRITE_SLOTS: usize = 8;
pub(super) const QUERY_SLOTS: usize = 64;
pub(super) const QUERY_START: Duration = Duration::from_secs(1);
pub(super) const QUERY_GATHER: Duration = Duration::from_millis(1);
pub(super) const DRAIN: Duration = Duration::from_secs(5);
pub(super) const BACKFILL_RETRY: Duration = Duration::from_secs(5);
pub(super) const BACKFILL_IDLE: Duration = Duration::from_secs(5);
pub(super) const SOCKET_BYTES: usize = 103;
pub(super) const REGISTRY_BYTES: usize = MIB;
pub(super) const REGISTRY_RECORDS: usize = 1024;

pub use comb_protocol::service::limits::{
    DEFAULT_GRAPH_DEPTH, DEFAULT_LINK_LIMIT, FRAME_READ, MIB, OUTPUT, REQUEST,
};
