//! Shared wire bounds and client deadlines.
use std::time::Duration;
pub const MIB: usize = 1024 * 1024;
pub const REQUEST_BYTES: usize = 8 * MIB;
pub const RESPONSE_BYTES: usize = 32 * MIB;
pub const MCP_BYTES: usize = 64 * MIB;
pub const FRAME_READ: Duration = Duration::from_secs(5);
pub const CONNECT: Duration = Duration::from_secs(5);
pub const REQUEST: Duration = Duration::from_secs(60);
pub const CAPABILITIES: Duration = Duration::from_secs(10);
pub const OUTPUT: Duration = Duration::from_secs(10);
pub const CREDENTIAL_BYTES: usize = 16 * 1024;
pub const REVIEWER_ROLES: usize = 64;
pub const DEFAULT_LINK_LIMIT: usize = 100;
pub const DEFAULT_GRAPH_DEPTH: usize = 2;
