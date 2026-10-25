//! Provider boundary classifications with stable public messages and typed local causes.
use crate::error::Error;

const INVALID_STATE: &str = "Invalid provider protocol state.";
const INVALID_EXCHANGE: &str = "Cannot exchange a valid provider protocol message.";
const INVALID_NATIVE_STATE: &str = "Invalid native Claude protocol state.";
const UNAVAILABLE: &str = "Provider connection is unavailable; uncertain input is not retried.";
pub(super) const ACK_TIMEOUT: &str = "Provider did not acknowledge the request in time.";
pub(super) const EXITED_BEFORE_CLOSE: &str = "Provider exited before the session was closed.";
pub(super) const CLEANING_UP: &str = "Provider is cleaning up.";
pub(super) const TERMINAL_QUIET_TIMEOUT: &str =
    "Terminal cleanup did not reach observed quiet in time.";
pub(super) const TERMINAL_CLEANUP_TIMEOUT: &str = "Terminal cleanup was not confirmed in time.";
pub(super) const THREAD_UNLOAD_TIMEOUT: &str = "Provider thread unload was not confirmed in time.";

pub(crate) fn protocol() -> Error {
    Error::provider_protocol_error(INVALID_STATE)
}
pub(super) fn transport_protocol() -> Error {
    Error::provider_protocol_error(INVALID_EXCHANGE)
}
pub(super) fn native_protocol() -> Error {
    Error::provider_protocol_error(INVALID_NATIVE_STATE)
}
pub(super) fn unavailable() -> Error {
    Error::provider_unavailable(UNAVAILABLE)
}

#[cfg(test)]
mod tests;
