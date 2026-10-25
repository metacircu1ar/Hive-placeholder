//! Signal and inspect owned process groups while preserving leader ownership.
// libproc's process-group filter, absent from the supported libc bindings.
#[cfg(target_os = "linux")]
use super::peer::pids;
use super::{Signal, invalid, no_such_process};
use rustix::process::{self as native_process, Pid};
#[cfg(target_os = "macos")]
use std::mem;
#[cfg(target_os = "linux")]
use std::{fs, ptr};
use std::{io, process::Child};
#[cfg(target_os = "macos")]
const PROC_PGRP_ONLY: u32 = 2;
// Grow the bounded PID snapshot until libproc confirms that all members fit.
#[cfg(target_os = "macos")]
const INITIAL_GROUP_MEMBERS: usize = 256;
#[cfg(target_os = "macos")]
const MAX_GROUP_MEMBERS: usize = 65_536;
#[cfg(target_os = "macos")]
const GROUP_GROWTH_FACTOR: usize = 2;
#[cfg(target_os = "macos")]
const _: () = assert!(INITIAL_GROUP_MEMBERS <= MAX_GROUP_MEMBERS);

/// The caller owns an unreaped leader created in its own process group.
pub(crate) fn signal_group(child: &Child, signal: Signal) -> io::Result<()> {
    native_process::kill_process_group(Pid::from_child(child), signal).map_err(Into::into)
}

/// Read-only probe: unlike group signalling this is safe after the leader has
/// been reaped. PID reuse can only make absence unconfirmed.
pub(crate) fn group_absent(pid: u32) -> io::Result<bool> {
    let pid = i32::try_from(pid)
        .ok()
        .filter(|p| *p > 1)
        .ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
    // SAFETY: kill with signal zero sends no signal and dereferences no pointers.
    if unsafe { libc::kill(-pid, 0) } == 0 {
        return Ok(false);
    }
    let error = io::Error::last_os_error();
    if no_such_process(&error) {
        Ok(true)
    } else if error.raw_os_error() == Some(libc::EPERM) {
        // A zombie-only group can return EPERM on macOS. It is still present;
        // callers retain their bounded wait rather than assuming absence.
        Ok(false)
    } else {
        Err(error)
    }
}

pub(crate) fn own_process_group() -> io::Result<()> {
    native_process::setpgid(None, None).map_err(Into::into)
}

/// Become the reaper of orphaned descendants. Without it they reach PID 1, and a
/// container's PID 1 may never reap them. Best effort: a refusal leaves reaping
/// to PID 1, as before.
#[cfg(target_os = "linux")]
pub(crate) fn adopt_orphans() {
    // A kernel without PR_SET_CHILD_SUBREAPER leaves the previous behavior.
    let _ = adopt_children();
}
#[cfg(target_os = "linux")]
pub(crate) fn adopt_children() -> io::Result<()> {
    // SAFETY: prctl takes scalar arguments for PR_SET_CHILD_SUBREAPER.
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } != 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
#[cfg(target_os = "linux")]
pub(crate) fn reap_adopted_group(pid: u32) -> io::Result<()> {
    let pid = i32::try_from(pid).map_err(|_| invalid())?;
    if pid <= 1 {
        return Err(invalid());
    }
    loop {
        // SAFETY: waitpid with null status discards status; WNOHANG cannot block.
        let found = unsafe { libc::waitpid(-pid, ptr::null_mut(), libc::WNOHANG) };
        if found == 0 {
            return Ok(());
        }
        if found < 0 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(libc::ECHILD) {
                Ok(())
            } else {
                Err(error)
            };
        }
    }
}

/// Used only while the caller still owns its unreaped group leader.
pub(crate) fn group_only_leader(leader: u32) -> io::Result<bool> {
    inspect_group(leader)
}

#[cfg(target_os = "macos")]
fn inspect_group(leader: u32) -> io::Result<bool> {
    let mut members = vec![0_i32; INITIAL_GROUP_MEMBERS];
    loop {
        let bytes = i32::try_from(members.len() * mem::size_of::<i32>()).map_err(|_| invalid())?;
        // SAFETY: PROC_PGRP_ONLY (2) writes PIDs into the sized output buffer.
        let count = unsafe {
            libc::proc_listpids(PROC_PGRP_ONLY, leader, members.as_mut_ptr().cast(), bytes)
        };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        if count == bytes {
            if members.len() >= MAX_GROUP_MEMBERS {
                return Err(invalid());
            }
            members.resize(members.len() * GROUP_GROWTH_FACTOR, 0);
            continue;
        }
        let count = usize::try_from(count).map_err(|_| invalid())? / mem::size_of::<i32>();
        return Ok(members[..count]
            .iter()
            .all(|pid| *pid == 0 || u32::try_from(*pid).ok() == Some(leader)));
    }
}

#[cfg(target_os = "linux")]
fn inspect_group(leader: u32) -> io::Result<bool> {
    let host_leader = host_leader(leader)?;
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        let Ok(host) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        if host == host_leader {
            continue;
        }
        let stat = match fs::read_to_string(entry.path().join("stat")) {
            Ok(stat) => stat,
            Err(e)
                if e.kind() == io::ErrorKind::NotFound || e.raw_os_error() == Some(libc::ESRCH) =>
            {
                continue;
            }
            Err(e) => return Err(e),
        };
        let mut fields = stat
            .rsplit_once(')')
            .ok_or_else(invalid)?
            .1
            .split_whitespace();
        let state = fields.next().ok_or_else(invalid)?;
        fields.next().ok_or_else(invalid)?; // parent PID
        let group: u32 = fields
            .next()
            .ok_or_else(invalid)?
            .parse()
            .map_err(|_| invalid())?;
        // Z may describe only an exited main thread. The final group-absence
        // probe remains authoritative even then. Reap adopted children only
        // after releasing the leader, preserving its PID through signals.
        if group == host_leader && !matches!(state, "Z" | "X") {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(target_os = "linux")]
fn host_leader(leader: u32) -> io::Result<u32> {
    // /proc may be mounted in the outer PID namespace. Resolve the still
    // owned direct child once, then compare stat's PGIDs in that same view.
    // This avoids ptrace-gated namespace links of unrelated sandbox PID 1.
    let own_status = fs::read_to_string("/proc/self/status")?;
    let own: Vec<u32> = own_status
        .lines()
        .find_map(|line| line.strip_prefix("NSpid:"))
        .ok_or_else(invalid)?
        .split_whitespace()
        .map(|value| value.parse().map_err(|_| invalid()))
        .collect::<io::Result<_>>()?;
    let host_self = own.first().ok_or_else(invalid)?;
    let children = fs::read_to_string(format!("/proc/self/task/{host_self}/children"))?;
    let mut host_leader = None;
    for child in children.split_whitespace() {
        let host: u32 = child.parse().map_err(|_| invalid())?;
        let ids = match pids(host) {
            Ok(ids) => ids,
            Err(e)
                if e.kind() == io::ErrorKind::NotFound || e.raw_os_error() == Some(libc::ESRCH) =>
            {
                continue;
            }
            Err(e) => return Err(e),
        };
        if ids.len() == own.len()
            && ids.last() == Some(&leader)
            && host_leader.replace(host).is_some()
        {
            return Err(invalid());
        }
    }
    host_leader.ok_or_else(invalid)
}
