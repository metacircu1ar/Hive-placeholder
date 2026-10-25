//! Independent limits for native callbacks, tool ownership and replay evidence.
// The cancellation ledger distinguishes late tool replies from unknown requests.
pub(super) const CANCELLED_CALLBACK_LIMIT: usize = 256;
// Bound native MCP callbacks waiting for a Hive tool result.
pub(super) const MCP_CALLBACK_LIMIT: usize = 64;
// Tool owners and active permissions share one concurrent native-tool capacity.
pub(super) const ACTIVE_TOOL_LIMIT: usize = 256;
// Hooks lack a turn discriminator; keep lifetime IDs without eviction to reject replay.
pub(super) const SEEN_TOOL_LIMIT: usize = 8192;
pub(super) const TOOL_ID_BYTES: usize = 1024;
// Bound provider-supplied tool names independently from model and request identifiers.
pub(super) const TOOL_NAME_BYTES: usize = 128;
// Bound the canary's native work even if its fixed read repeats.
pub(super) const CANARY_TOOL_LIMIT: usize = 8;
// Retain exact emitted replies until their native echo settles them.
pub(super) const REPLY_ECHO_LIMIT: usize = 256;
pub(super) const REPLY_ECHO_BYTES: usize = 2 * crate::limits::JOIN_TRANSPORT_BYTES;
// Bound aggregation of split native assistant text before emitting one item.
pub(super) const ASSISTANT_TEXT_LIMIT: usize = 4 * 1024 * 1024;
const _: () = assert!(ACTIVE_TOOL_LIMIT <= SEEN_TOOL_LIMIT);
// The public output-limit message states four MiB.
const _: () = assert!(ASSISTANT_TEXT_LIMIT == 4 * 1024 * 1024);
