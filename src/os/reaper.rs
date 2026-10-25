//! Linux child subreaping. A process that adopts orphaned descendants must reap
//! the ones that exit, or a zombie keeps its process group present and cleanup
//! never confirms. Only children this process did not spawn are reaped: every
//! spawn registers its child, so an owner's exit status is never taken.
use super::process::try_fork_guard;
use std::{fs, io, mem, sync::Mutex};

/// Registry size that triggers pruning of children that no longer exist.
const PRUNE_AT: usize = 1024;
/// Field index of the start time after the command name in `/proc/PID/stat`.
const START_TIME_FIELD: usize = 19;

/// Children spawned by this process: (PID, start time in clock ticks).
static SPAWNED: Mutex<Vec<(u32, u64)>> = Mutex::new(Vec::new());
/// Whether `/proc` numbers processes as this process sees them.
static OWN_NAMESPACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// `/proc` may belong to another PID namespace, as for a container given the
/// host's: its numbers then name other processes, so nothing is reaped.
fn own_namespace() -> bool {
    *OWN_NAMESPACE.get_or_init(|| {
        fs::read_to_string("/proc/self/status").is_ok_and(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("NSpid:"))
                .and_then(|pids| pids.split_whitespace().next())
                .and_then(|pid| pid.parse::<u32>().ok())
                == Some(std::process::id())
        })
    })
}

/// The fields of `/proc/PID/stat` reaping needs.
struct Stat {
    state: u8,
    parent: u32,
    start: u64,
}
fn stat(pid: u32) -> Option<Stat> {
    inspect(pid).ok().flatten()
}
fn inspect(pid: u32) -> io::Result<Option<Stat>> {
    let text = match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(text) => text,
        Err(error) if matches!(error.raw_os_error(), Some(libc::ENOENT | libc::ESRCH)) => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    parse_stat(&text)
        .map(Some)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid process metadata"))
}
fn parse_stat(text: &str) -> Option<Stat> {
    let fields: Vec<&str> = text.rsplit_once(')')?.1.split_whitespace().collect();
    Some(Stat {
        state: *fields.first()?.as_bytes().first()?,
        parent: fields.get(1)?.parse().ok()?,
        start: fields.get(START_TIME_FIELD)?.parse().ok()?,
    })
}

/// Record a child just spawned. Callers hold the launch lock, so no reaper runs
/// before the child is known.
pub(crate) fn register(pid: u32) {
    let start = stat(pid).map_or(0, |stat| stat.start);
    let Ok(mut spawned) = SPAWNED.lock() else {
        return;
    };
    if spawned.len() >= PRUNE_AT {
        spawned.retain(|&(pid, start)| retain_owned(start, inspect(pid)));
    }
    spawned.push((pid, start));
}
/// Only proven disappearance or PID reuse retires an owner's protection.
fn retain_owned(start: u64, observed: io::Result<Option<Stat>>) -> bool {
    match observed {
        Ok(None) => false,
        Ok(Some(current)) => start == 0 || current.start == start,
        Err(_) => true,
    }
}
fn spawned(pid: u32, start: u64) -> bool {
    // An unreadable registry protects everything: reaping is only an optimization.
    SPAWNED.lock().map_or(true, |spawned| {
        spawned
            .iter()
            .any(|&(known, started)| known == pid && (started == start || started == 0))
    })
}

/// The PID of a child in `id` that has exited and is still waitable, without
/// reaping it.
fn exited(kind: libc::idtype_t, id: u32) -> io::Result<Option<u32>> {
    loop {
        // SAFETY: siginfo_t is a plain C structure; zero is a valid initial value.
        let mut info: libc::siginfo_t = unsafe { mem::zeroed() };
        // SAFETY: waitid writes only the provided siginfo; WNOWAIT keeps the
        // child waitable and WNOHANG never blocks.
        let result = unsafe {
            libc::waitid(
                kind,
                libc::id_t::from(id),
                &raw mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result != 0 {
            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::EINTR) => continue,
                Some(libc::ECHILD) => return Ok(None),
                _ => return Err(error),
            }
        }
        // SAFETY: waitid filled the structure; si_pid is valid for child events.
        let pid = unsafe { info.si_pid() };
        return Ok(u32::try_from(pid).ok().filter(|pid| *pid > 0));
    }
}
/// Reap `pid` if it is an exited child this process did not spawn.
fn reap_unspawned(pid: u32) -> io::Result<bool> {
    let Some(stat) = stat(pid) else {
        return Ok(false);
    };
    if stat.state != b'Z' || spawned(pid, stat.start) {
        return Ok(false);
    }
    // SAFETY: a null status is accepted; WNOHANG cannot block.
    let reaped = unsafe {
        libc::waitpid(
            libc::pid_t::try_from(pid).map_err(io::Error::other)?,
            std::ptr::null_mut(),
            libc::WNOHANG,
        )
    };
    if reaped < 0 {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(libc::ECHILD) {
            Ok(false)
        } else {
            Err(error)
        };
    }
    Ok(reaped > 0)
}

/// Reap exited members of process group `group` that this process adopted.
/// Stops at a spawned child: its owner reaps it.
pub(crate) fn reap_group(group: u32) -> io::Result<()> {
    if !own_namespace() {
        return Ok(());
    }
    // While a launch is in progress, a new child may not be registered yet.
    let Some(_fork) = try_fork_guard() else {
        return Ok(());
    };
    while let Some(pid) = exited(libc::P_PGID, group)? {
        if !reap_unspawned(pid)? {
            break;
        }
    }
    Ok(())
}
/// Reap the adopted child `pid` if it has exited and started at `start`.
pub(crate) fn reap_process(pid: u32, start: u64) -> io::Result<()> {
    if !own_namespace() {
        return Ok(());
    }
    let Some(_fork) = try_fork_guard() else {
        return Ok(());
    };
    if stat(pid).is_some_and(|stat| stat.start == start) && exited(libc::P_PID, pid)?.is_some() {
        reap_unspawned(pid)?;
    }
    Ok(())
}
/// Reap every exited child this process adopted. Returns how many were reaped.
pub(crate) fn reap_adopted() -> io::Result<usize> {
    if !own_namespace() {
        return Ok(0);
    }
    // Find exited children first, without holding up launches for the walk.
    let me = std::process::id();
    let mut exited = Vec::new();
    for entry in fs::read_dir("/proc")? {
        let Some(pid) = entry?
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if stat(pid).is_some_and(|stat| stat.parent == me && stat.state == b'Z') {
            exited.push(pid);
        }
    }
    if exited.is_empty() {
        return Ok(0);
    }
    let Some(_fork) = try_fork_guard() else {
        return Ok(0);
    };
    let mut reaped = 0;
    for pid in exited {
        // Checked again under the lock: reap_unspawned rereads its state.
        if reap_unspawned(pid)? {
            reaped += 1;
        }
    }
    Ok(reaped)
}

#[cfg(test)]
mod tests;
