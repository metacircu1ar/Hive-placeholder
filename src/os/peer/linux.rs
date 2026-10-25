//! Read Linux executable and PID namespace identities from procfs.
use super::invalid;
use std::{fs, io, path::PathBuf};

#[cfg(target_os = "linux")]
#[expect(
    clippy::unnecessary_wraps,
    reason = "The shared peer verifier also uses fallible macOS executable lookup."
)]
pub(super) fn executable(pid: u32) -> io::Result<PathBuf> {
    // Stat the kernel executable reference itself, not a pathname resolved in
    // the daemon's potentially different mount namespace.
    Ok(PathBuf::from(format!("/proc/{pid}/exe")))
}
#[cfg(target_os = "linux")]
pub(in crate::os) fn pids(pid: u32) -> io::Result<Vec<u32>> {
    let text = fs::read_to_string(format!("/proc/{pid}/status"))?;
    text.lines()
        .find_map(|l| l.strip_prefix("NSpid:"))
        .ok_or_else(invalid)?
        .split_whitespace()
        .map(|s| s.parse().map_err(|_| invalid()))
        .collect()
}
