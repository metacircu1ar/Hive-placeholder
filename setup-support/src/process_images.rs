//! Conservative current-user executable scan for explicitly requested image purge.
use crate::{Result, invalid};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

/// Most files an installation image may hold for the scan to compare.
const MAX_IMAGE_FILES: usize = 200_000;

/// An executable or mapped file of a live process: its path, and its file
/// identity (device, inode) when known. A deleted file keeps both.
type Use = (u32, PathBuf, Option<(u64, u64)>);

pub fn ensure_unused(roots: &[&Path]) -> Result<()> {
    let files = image_files(roots)?;
    for (pid, image, identity) in images()? {
        if pid != std::process::id()
            && (roots.iter().any(|root| image.starts_with(root))
                || identity.is_some_and(|identity| files.contains(&identity)))
        {
            return Err(invalid(format!(
                "Installation image is still used by PID {pid}: {}. Stop that process \
                    before retrying purge.",
                image.display()
            )));
        }
    }
    Ok(())
}
/// The (device, inode) of every regular file under the roots, so a process
/// running one through a hard link elsewhere is still found.
fn image_files(roots: &[&Path]) -> Result<HashSet<(u64, u64)>> {
    use std::os::unix::fs::MetadataExt;
    let mut files = HashSet::new();
    let mut pending: Vec<PathBuf> = roots.iter().map(|root| root.to_path_buf()).collect();
    while let Some(path) = pending.pop() {
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if meta.is_dir() {
            for entry in std::fs::read_dir(&path)? {
                pending.push(entry?.path());
            }
        } else if meta.is_file() {
            files.insert((meta.dev(), meta.ino()));
            if files.len() > MAX_IMAGE_FILES {
                return Err(invalid("Installation image has too many files to scan."));
            }
        }
    }
    Ok(files)
}
#[cfg(target_os = "linux")]
fn images() -> Result<Vec<Use>> {
    use std::{
        fs,
        os::unix::{
            ffi::{OsStrExt, OsStringExt},
            fs::MetadataExt,
        },
    };
    let uid = rustix::process::geteuid().as_raw();
    let mut result = vec![];
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let path = entry.path();
        let meta = match fs::metadata(&path) {
            Ok(meta) => meta,
            Err(e) if gone(&e) => continue,
            Err(e) => return Err(e),
        };
        if meta.uid() != uid || pid == std::process::id() {
            continue;
        }
        match fs::read_link(path.join("exe")) {
            Ok(image) => {
                let bytes = image.as_os_str().as_bytes();
                let bytes = bytes.strip_suffix(b" (deleted)").unwrap_or(bytes);
                // The link resolves to the running inode, even when deleted.
                let identity = match fs::metadata(path.join("exe")) {
                    Ok(meta) => Some((meta.dev(), meta.ino())),
                    Err(e) if gone(&e) => None,
                    Err(e) => return Err(e),
                };
                result.push((
                    pid,
                    PathBuf::from(std::ffi::OsString::from_vec(bytes.to_vec())),
                    identity,
                ));
            }
            Err(e) if gone(&e) => {
                let stat = match fs::read_to_string(path.join("stat")) {
                    Ok(s) => s,
                    Err(e) if gone(&e) => continue,
                    Err(e) => return Err(e),
                };
                let rest = stat
                    .rsplit_once(") ")
                    .ok_or_else(|| invalid("Malformed process identity."))?
                    .1;
                let fields: Vec<_> = rest.split_whitespace().collect();
                let flags = fields
                    .get(6)
                    .and_then(|s| s.parse::<u64>().ok())
                    .ok_or_else(|| invalid("Malformed process flags."))?;
                if fields.first() != Some(&"Z") && flags & 0x0020_0000 == 0 {
                    return Err(invalid(format!(
                        "Cannot inspect live PID {pid}; image purge refused."
                    )));
                }
            }
            Err(e) => return Err(e),
        }
    }
    Ok(result)
}
fn gone(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::ENOENT | libc::ESRCH))
}

#[cfg(target_os = "macos")]
fn images() -> Result<Vec<Use>> {
    use std::os::unix::ffi::OsStringExt;
    const PROC_UID_ONLY: u32 = 4;
    let mut pids = vec![0_i32; 131_072];
    let bytes = i32::try_from(pids.len() * std::mem::size_of::<i32>())
        .map_err(|_| invalid("Process scan overflow."))?;
    // SAFETY: writable allocation and byte count agree; UID-only never inspects other users.
    let count = unsafe {
        libc::proc_listpids(
            PROC_UID_ONLY,
            rustix::process::geteuid().as_raw(),
            pids.as_mut_ptr().cast(),
            bytes,
        )
    };
    if count <= 0 || count >= bytes {
        return Err(invalid(
            "Could not obtain a complete current-user process inventory.",
        ));
    }
    let count = usize::try_from(count).map_err(|_| invalid("Invalid process count."))?
        / std::mem::size_of::<i32>();
    let mut result = vec![];
    for &pid in &pids[..count] {
        if pid <= 0 || u32::try_from(pid).ok() == Some(std::process::id()) {
            continue;
        }
        let mut buffer = vec![0_u8; 4096];
        // SAFETY: writable byte buffer with the declared capacity.
        let size = unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), 4096) };
        if size <= 0 {
            let error = std::io::Error::last_os_error();
            result.extend(unavailable_image(pid, &error)?);
            continue;
        }
        buffer.truncate(buffer.iter().position(|b| *b == 0).unwrap_or(buffer.len()));
        let image = PathBuf::from(std::ffi::OsString::from_vec(buffer));
        // The file at that path now; a hard link to an image file shares its inode.
        let identity = std::fs::metadata(&image).ok().map(|meta| {
            use std::os::unix::fs::MetadataExt;
            (meta.dev(), meta.ino())
        });
        result.push((
            u32::try_from(pid).map_err(|_| invalid("Invalid process ID."))?,
            image,
            identity,
        ));
    }
    Ok(result)
}

/// A live process whose executable path is unavailable. An exited process, or
/// another user's, is skipped. One whose executable was deleted, such as an
/// upgraded program, is described by its mapped files. Anything else refuses.
#[cfg(target_os = "macos")]
fn unavailable_image(pid: i32, error: &std::io::Error) -> Result<Vec<Use>> {
    // ENOENT can describe an unavailable executable vnode, not process exit.
    // Confirm liveness/ownership independently before skipping this PID.
    let status = bsd_info(pid);
    let start = match &status {
        Ok(Some((_, _, start))) => Some(*start),
        _ => None,
    };
    if missing_image_is_safe(status.map(|found| found.map(|(uid, state, _)| (uid, state))))? {
        return Ok(Vec::new());
    }
    finish_unavailable_image(
        pid,
        error,
        start.and_then(|start| mapped_files(pid, start)),
        || bsd_info(pid).map(|found| found.map(|(uid, state, _)| (uid, state))),
    )
}

#[cfg(target_os = "macos")]
fn finish_unavailable_image(
    pid: i32,
    error: &std::io::Error,
    mapped: Option<Vec<Use>>,
    current: impl FnOnce() -> Result<Option<(u32, u32)>>,
) -> Result<Vec<Use>> {
    if let Some(mapped) = mapped {
        return Ok(mapped);
    }
    // The process can exit while its regions are being walked. Only fresh,
    // independent exit/owner evidence permits skipping an incomplete walk.
    if missing_image_is_safe(current())? {
        return Ok(Vec::new());
    }
    Err(invalid(format!(
        "Cannot inspect live PID {pid}: {error}; image purge refused."
    )))
}
/// `PROC_PIDREGIONPATHINFO`: a region at or after an address, with its file.
#[cfg(target_os = "macos")]
const PROC_PIDREGIONPATHINFO: libc::c_int = 8;
/// `struct proc_regioninfo` from `<sys/proc_info.h>`.
#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
#[allow(clippy::struct_field_names)]
struct RegionInfo {
    pri_protection: u32,
    pri_max_protection: u32,
    pri_inheritance: u32,
    pri_flags: u32,
    pri_offset: u64,
    pri_behavior: u32,
    pri_user_wired_count: u32,
    pri_user_tag: u32,
    pri_pages_resident: u32,
    pri_pages_shared_now_private: u32,
    pri_pages_swapped_out: u32,
    pri_pages_dirtied: u32,
    pri_ref_count: u32,
    pri_shadow_depth: u32,
    pri_share_mode: u32,
    pri_private_pages_resident: u32,
    pri_shared_pages_resident: u32,
    pri_obj_id: u32,
    pri_depth: u32,
    pri_address: u64,
    pri_size: u64,
}
/// `struct proc_regionwithpathinfo`.
#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
struct RegionWithPath {
    region: RegionInfo,
    file: libc::vnode_info_path,
}
/// Every file-backed region of `pid`, or `None` when the walk is incomplete,
/// finds no file, or the PID now names another process (`start` differs).
#[cfg(target_os = "macos")]
fn mapped_files(pid: i32, start: (u64, u64)) -> Option<Vec<Use>> {
    use std::os::unix::ffi::OsStrExt;
    let owner = u32::try_from(pid).ok()?;
    let size = i32::try_from(std::mem::size_of::<RegionWithPath>()).ok()?;
    let mut mapped = Vec::new();
    let mut address = 0_u64;
    loop {
        let mut region = std::mem::MaybeUninit::<RegionWithPath>::uninit();
        // SAFETY: the buffer has exactly the requested size; read only after a full write.
        let got = unsafe {
            libc::proc_pidinfo(
                pid,
                PROC_PIDREGIONPATHINFO,
                address,
                region.as_mut_ptr().cast(),
                size,
            )
        };
        if got != size {
            // The walk ends with EINVAL past the last region; anything else is unknown.
            if got == 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINVAL) {
                break;
            }
            return None;
        }
        // SAFETY: the kernel wrote the complete structure.
        let region = unsafe { region.assume_init() };
        let file_stat = region.file.vip_vi.vi_stat;
        if file_stat.vst_ino != 0 {
            // SAFETY: vip_path is a NUL-terminated C array of MAXPATHLEN bytes.
            let path = unsafe { std::ffi::CStr::from_ptr(region.file.vip_path.as_ptr().cast()) };
            mapped.push((
                owner,
                PathBuf::from(std::ffi::OsStr::from_bytes(path.to_bytes())),
                Some((u64::from(file_stat.vst_dev), file_stat.vst_ino)),
            ));
        }
        let next = region
            .region
            .pri_address
            .checked_add(region.region.pri_size)?;
        if next <= address {
            return None;
        }
        address = next;
    }
    // The same process from start to end of the walk, with files to compare.
    let again = bsd_info(pid).ok().flatten().map(|(_, _, again)| again);
    (again == Some(start) && !mapped.is_empty()).then_some(mapped)
}
/// A process's owner, status and start time (seconds, microseconds).
#[cfg(target_os = "macos")]
type BsdInfo = (u32, u32, (u64, u64));
/// The process's `BsdInfo`; `None` once it has exited.
#[cfg(target_os = "macos")]
fn bsd_info(pid: i32) -> Result<Option<BsdInfo>> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let expected = i32::try_from(std::mem::size_of::<libc::proc_bsdinfo>())
        .map_err(|_| invalid("Process info overflow."))?;
    // SAFETY: buffer has exactly the requested struct size; assume_init only after full write.
    let got = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            expected,
        )
    };
    if got == expected {
        // SAFETY: the kernel wrote the complete proc_bsdinfo.
        let info = unsafe { info.assume_init() };
        Ok(Some((
            info.pbi_uid,
            info.pbi_status,
            (info.pbi_start_tvsec, info.pbi_start_tvusec),
        )))
    } else if got <= 0 && gone(&std::io::Error::last_os_error()) {
        Ok(None)
    } else {
        Err(invalid("Incomplete process identity."))
    }
}

#[cfg(target_os = "macos")]
fn missing_image_is_safe(status: Result<Option<(u32, u32)>>) -> Result<bool> {
    Ok(status?.is_none_or(|(uid, state)| uid != rustix::process::geteuid().as_raw() || state == 5))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    #[cfg(target_os = "macos")]
    fn exit_during_region_walk_requires_fresh_evidence() {
        let uid = rustix::process::geteuid().as_raw();
        let error = std::io::Error::from_raw_os_error(libc::ENOENT);
        for final_state in [None, Some((uid, 5)), Some((uid + 1, 2))] {
            assert!(
                finish_unavailable_image(123, &error, None, || Ok(final_state))
                    .unwrap()
                    .is_empty()
            );
        }
        let live = finish_unavailable_image(123, &error, None, || Ok(Some((uid, 2))));
        assert!(
            live.unwrap_err()
                .to_string()
                .contains("image purge refused")
        );
        let unknown = finish_unavailable_image(123, &error, None, || {
            Err(std::io::Error::from_raw_os_error(libc::EACCES))
        });
        assert_eq!(unknown.unwrap_err().raw_os_error(), Some(libc::EACCES));
    }
    #[test]
    #[cfg(target_os = "macos")]
    fn unavailable_path_requires_independent_exit_or_owner_evidence() {
        let uid = rustix::process::geteuid().as_raw();
        assert!(!missing_image_is_safe(Ok(Some((uid, 2)))).unwrap());
        assert!(missing_image_is_safe(Ok(None)).unwrap());
        assert!(missing_image_is_safe(Ok(Some((uid, 5)))).unwrap());
        assert!(missing_image_is_safe(Ok(Some((uid + 1, 2)))).unwrap());
        assert!(
            missing_image_is_safe(Err(std::io::Error::from_raw_os_error(libc::EACCES))).is_err()
        );
    }
    /// A sleeping process, killed when dropped even if its test fails.
    struct Sleeper(std::process::Child);
    impl Drop for Sleeper {
        fn drop(&mut self) {
            // An already ended process is fine; there is nothing else to clean.
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    /// A running copy of this test binary, sleeping in `sleeping_fixture`.
    fn sleeping_copy(dir: &Path) -> (Sleeper, PathBuf) {
        let path = dir.join("sleeper");
        std::fs::copy(std::env::current_exe().unwrap(), &path).unwrap();
        let ready = dir.join("ready");
        let child = std::process::Command::new(&path)
            .args([
                "--exact",
                "process_images::tests::sleeping_fixture",
                "--ignored",
            ])
            .env("SLEEPER_READY", &ready)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let child = Sleeper(child);
        // A fresh copy's first run can be slow while macOS assesses it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        while !ready.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "sleeper never started"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        (child, path)
    }
    #[test]
    #[ignore = "a long-running program for the purge scan tests"]
    fn sleeping_fixture() {
        std::fs::write(std::env::var_os("SLEEPER_READY").unwrap(), b"").unwrap();
        std::thread::sleep(std::time::Duration::from_secs(30));
    }
    #[test]
    fn a_deleted_executable_blocks_purge_only_of_its_own_image() {
        let image = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let (child, path) = sleeping_copy(image.path());
        // As after an upgrade: the program runs on with its file deleted.
        std::fs::remove_file(&path).unwrap();
        let image_root = image.path().canonicalize().unwrap();
        let other_root = other.path().canonicalize().unwrap();
        let own = ensure_unused(&[&image_root]);
        let unrelated = ensure_unused(&[&other_root]);
        let pid = child.0.id();
        drop(child);
        let error = own.unwrap_err().to_string();
        assert!(error.contains(&format!("PID {pid}")), "{error}");
        unrelated.unwrap();
    }
    #[test]
    fn a_hard_link_into_the_image_is_found_by_its_inode() {
        let image = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let (child, path) = sleeping_copy(outside.path());
        std::fs::hard_link(&path, image.path().join("linked")).unwrap();
        let result = ensure_unused(&[&image.path().canonicalize().unwrap()]);
        let pid = child.0.id();
        drop(child);
        let error = result.unwrap_err().to_string();
        assert!(error.contains(&format!("PID {pid}")), "{error}");
    }
    #[test]
    fn live_owned_image_is_reported_then_releases_after_exit() {
        let temp = tempfile::tempdir().unwrap();
        let image = temp.path().join("sleep");
        std::fs::copy("/bin/sleep", &image).unwrap();
        let mut child = std::process::Command::new(&image)
            .arg("20")
            .spawn()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        let result = ensure_unused(&[&root]);
        child.kill().unwrap();
        child.wait().unwrap();
        let error = result.unwrap_err().to_string();
        assert!(error.contains(&format!("PID {}", child.id())), "{error}");
        ensure_unused(&[&root]).unwrap();
    }
}
