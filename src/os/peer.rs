//! Authenticate shell peers and direct child identities from kernel process metadata.
// Bound parent traversal even when process metadata changes during inspection.
#[cfg(target_os = "linux")]
use super::effective_uid;
use super::invalid;
#[cfg(target_os = "linux")]
use linux::executable;
#[cfg(target_os = "macos")]
use macos::{executable, info};
#[cfg(target_os = "linux")]
use std::process::id as process_id;
use std::{
    fs, io, mem,
    os::{
        fd::AsRawFd,
        unix::{fs::MetadataExt, net::UnixStream},
    },
    path::Path,
};
const MAX_ANCESTORS: usize = 64;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "linux")]
pub(super) use linux::pids;

pub(crate) struct Peer {
    pub(crate) pid: u32,
    pub(crate) local_pid: u32,
}
#[derive(PartialEq, Eq)]
struct ParentGroup {
    parent: u32,
    group: u32,
}
fn parent_group(pid: u32) -> io::Result<ParentGroup> {
    #[cfg(target_os = "macos")]
    {
        let b = info(pid)?;
        Ok(ParentGroup {
            parent: b.pbi_ppid,
            group: b.pbi_pgid,
        })
    }
    #[cfg(target_os = "linux")]
    {
        let text = fs::read_to_string(format!("/proc/{pid}/stat"))?;
        let mut fields = text
            .rsplit_once(')')
            .ok_or_else(invalid)?
            .1
            .split_whitespace();
        fields.next();
        let mut next = || {
            fields
                .next()
                .ok_or_else(invalid)?
                .parse()
                .map_err(|_| invalid())
        };
        Ok(ParentGroup {
            parent: next()?,
            group: next()?,
        })
    }
}
#[cfg(target_os = "macos")]
fn peer_pid(stream: &UnixStream) -> io::Result<u32> {
    let mut pid: libc::pid_t = 0;
    let mut size = libc::socklen_t::try_from(mem::size_of_val(&pid)).map_err(|_| invalid())?;
    // SAFETY: LOCAL_PEERPID (2) returns a pid_t to the valid output buffer.
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&raw mut pid).cast(),
            &raw mut size,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    u32::try_from(pid).map_err(|_| invalid())
}

#[cfg(target_os = "linux")]
fn peer_pid(stream: &UnixStream) -> io::Result<u32> {
    if pids(process_id())? != [process_id()] {
        return Err(invalid());
    }
    // SAFETY: ucred is a plain C output structure.
    let mut cred: libc::ucred = unsafe { mem::zeroed() };
    let mut size = libc::socklen_t::try_from(mem::size_of_val(&cred)).map_err(|_| invalid())?;
    // SAFETY: SO_PEERCRED writes a ucred to the valid buffer.
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut cred).cast(),
            &raw mut size,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    if cred.uid != effective_uid() {
        return Err(invalid());
    }
    u32::try_from(cred.pid).map_err(|_| invalid())
}
pub(crate) fn peer(stream: &UnixStream, expected: &Path, provider: u32) -> io::Result<Peer> {
    let pid = peer_pid(stream)?;
    let observed = fs::metadata(executable(pid)?)?;
    let expected = fs::metadata(expected)?;
    if observed.dev() != expected.dev() || observed.ino() != expected.ino() {
        return Err(invalid());
    }
    let mut ancestor = pid;
    for _ in 0..MAX_ANCESTORS {
        if ancestor == provider {
            #[cfg(target_os = "linux")]
            let local_pid = {
                let values = pids(pid)?;
                if values.first() != Some(&pid) {
                    return Err(invalid());
                }
                *values.last().ok_or_else(invalid)?
            };
            #[cfg(target_os = "macos")]
            let local_pid = pid;
            return Ok(Peer { pid, local_pid });
        }
        if ancestor <= 1 {
            break;
        }
        ancestor = parent_group(ancestor)?.parent;
    }
    Err(invalid())
}
pub(crate) fn child(peer: &Peer, local: u32) -> io::Result<u32> {
    #[cfg(target_os = "macos")]
    let host = local;
    #[cfg(target_os = "linux")]
    let host = {
        let candidates =
            fs::read_to_string(format!("/proc/{}/task/{}/children", peer.pid, peer.pid))?;
        let peers = pids(peer.pid)?;
        let ns = fs::metadata(format!("/proc/{}/ns/pid", peer.pid))?;
        let mut found = None;
        for pid in candidates.split_whitespace() {
            let pid: u32 = pid.parse().map_err(|_| invalid())?;
            let values = pids(pid)?;
            if values.len() == peers.len()
                && values.first() == Some(&pid)
                && values.last() == Some(&local)
            {
                let child_ns = fs::metadata(format!("/proc/{pid}/ns/pid"))?;
                if child_ns.dev() != ns.dev()
                    || child_ns.ino() != ns.ino()
                    || found.replace(pid).is_some()
                {
                    return Err(invalid());
                }
            }
        }
        found.ok_or_else(invalid)?
    };
    if host <= 1
        || parent_group(host)?
            != (ParentGroup {
                parent: peer.pid,
                group: host,
            })
    {
        return Err(invalid());
    }
    Ok(host)
}
