//! Shared wire errors and their private diagnostic causes.
pub use comb_protocol::error::*;

/// Preserve hook errors and all native publication/cleanup evidence privately.
pub(crate) fn publication(cause: atomic_fs::Error) -> Error {
    let mut error = cause
        .io_error()
        .get_ref()
        .and_then(|source| source.downcast_ref::<Error>())
        .cloned()
        .unwrap_or_else(|| Error::from(std::io::Error::from(cause.io_error().kind())));
    error.file_committed |= cause.published();
    error.caused_by(cause)
}
