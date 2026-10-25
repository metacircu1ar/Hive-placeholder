//! Expose kernel-derived shell peer, socket, and child-group identity checks.
pub(crate) use super::group::group_only_leader;
#[cfg(target_os = "linux")]
pub(crate) use super::group::{adopt_children, reap_adopted_group};
pub(crate) use super::peer::{Peer, child, peer};
pub(crate) use super::socket::socket_peer;
#[cfg(target_os = "linux")]
pub(crate) use super::socket::{connect_inherited, reuse_unconnected, unconnected_socket};

#[cfg(all(test, target_os = "macos"))]
mod tests;
