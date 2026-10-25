//! Checked native item identity; password bytes never cross this interface.
use std::{ffi::CString, io};
const NOT_FOUND: i32 = -25300;
pub(crate) const MAX_REFERENCE: usize = 4096;
unsafe extern "C" {
    fn hive_keychain_exists(bytes: *const u8, length: usize) -> i32;
    fn hive_keychain_reference(
        service: *const libc::c_char,
        account: *const libc::c_char,
        bytes: *mut u8,
        capacity: usize,
        length: *mut usize,
    ) -> i32;
    fn hive_keychain_delete(
        service: *const libc::c_char,
        account: *const libc::c_char,
        bytes: *const u8,
        length: usize,
    ) -> i32;
}
#[derive(Debug, thiserror::Error)]
#[error("Native Keychain metadata operation failed ({0}).")]
struct NativeError(i32);
fn error(status: i32) -> io::Error {
    io::Error::other(NativeError(status))
}
fn text(value: &str) -> io::Result<CString> {
    CString::new(value).map_err(|cause| io::Error::new(io::ErrorKind::InvalidInput, cause))
}
pub(crate) fn lookup(service: &str, account: &str) -> io::Result<Option<Vec<u8>>> {
    let (service, account) = (text(service)?, text(account)?);
    let mut bytes = vec![0; MAX_REFERENCE];
    let mut length = 0;
    // SAFETY: input strings are NUL-terminated and live across the synchronous call;
    // the helper writes at most capacity bytes and a valid length into owned storage.
    let status = unsafe {
        hive_keychain_reference(
            service.as_ptr(),
            account.as_ptr(),
            bytes.as_mut_ptr(),
            bytes.len(),
            &raw mut length,
        )
    };
    match status {
        0 if length > 0 && length <= bytes.len() => {
            bytes.truncate(length);
            Ok(Some(bytes))
        }
        NOT_FOUND => Ok(None),
        other => Err(error(other)),
    }
}
pub(crate) fn delete(service: &str, account: &str, reference: &[u8]) -> io::Result<()> {
    if reference.is_empty() || reference.len() > MAX_REFERENCE {
        return Err(super::invalid());
    }
    let (service, account) = (text(service)?, text(account)?);
    // SAFETY: strings and reference are live for the synchronous call. The C helper
    // copies reference bytes and only deletes that persistent item with these attributes.
    let status = unsafe {
        hive_keychain_delete(
            service.as_ptr(),
            account.as_ptr(),
            reference.as_ptr(),
            reference.len(),
        )
    };
    if status == 0 || status == NOT_FOUND {
        Ok(())
    } else {
        Err(error(status))
    }
}

pub(crate) fn exists(reference: &[u8]) -> io::Result<bool> {
    if reference.is_empty() || reference.len() > MAX_REFERENCE {
        return Err(super::invalid());
    }
    // SAFETY: the bounded live slice is copied by the synchronous native query;
    // the helper requests existence only and never password data.
    match unsafe { hive_keychain_exists(reference.as_ptr(), reference.len()) } {
        0 => Ok(true),
        NOT_FOUND => Ok(false),
        status => Err(error(status)),
    }
}
#[cfg(test)]
mod tests;
