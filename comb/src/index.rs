//! Exact-schema, transactionally replaced derived text index.
use rusqlite::Connection;
use std::sync::Arc;

pub(crate) mod cache;
mod chunks;
mod embedding;
mod embedding_write;
pub(crate) mod limits;
mod links;
mod pages;
mod schema;
pub(crate) mod search;
mod snapshot;

pub(crate) const SCHEMA: &str = include_str!("index/schema.sql");
pub struct Index {
    connection: Connection,
    vectors: Option<Arc<cache::Resident>>,
}

#[cfg(test)]
impl Index {
    pub(crate) fn has_resident_vectors(&self) -> bool {
        self.vectors.is_some()
    }
    // Read-only SQL observations keep their exact projections and ordering visible.
    pub(crate) fn test_connection(&self) -> &Connection {
        &self.connection
    }
}

#[cfg(test)]
mod test_support;
