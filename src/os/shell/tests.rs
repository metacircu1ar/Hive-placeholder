//! Verify that shell socket evidence accepts only the actual peer descriptor.
use super::*;
use std::os::{fd::AsRawFd, unix::net::UnixStream};
#[test]
fn socket_identity_accepts_only_the_actual_peer_descriptor() {
    let (left, right) = UnixStream::pair().unwrap();
    let (other, _) = UnixStream::pair().unwrap();
    assert!(socket_peer(&left, std::process::id(), right.as_raw_fd()).is_ok());
    assert!(socket_peer(&right, std::process::id(), left.as_raw_fd()).is_ok());
    assert!(socket_peer(&left, std::process::id(), other.as_raw_fd()).is_err());
    assert!(socket_peer(&left, std::process::id(), -1).is_err());
}
