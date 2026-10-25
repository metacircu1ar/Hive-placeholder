//! JSON-RPC codes interpreted or emitted at the provider protocol boundary.
// JSON-RPC's reserved request and server error codes keep native classifications explicit.
pub(crate) const INVALID_REQUEST: i32 = -32600;
pub(crate) const METHOD_NOT_FOUND: i32 = -32601;
pub(crate) const INVALID_PARAMS: i32 = -32602;
pub(crate) const INTERNAL_ERROR: i32 = -32603;
pub(crate) const SERVER_ERROR: i32 = -32000;
