//! Descriptor-based bounded reads and ownership/name checks.
use crate::{Error, Result};
use std::{
    fs::{self, File},
    os::unix::fs::MetadataExt,
    path::Path,
};
mod stable;
pub fn read(path: &Path, max: usize) -> Result<Vec<u8>> {
    stable::read(path, max)
}
#[cfg(test)]
mod tests;
pub fn check_file(meta: &fs::Metadata) -> Result<()> {
    if !meta.is_file()
        || meta.file_type().is_symlink()
        || meta.nlink() != 1
        || meta.uid() != rustix::process::geteuid().as_raw()
        || meta.mode() & 0o022 != 0
    {
        return Err(Error::unsafe_file());
    }
    Ok(())
}
pub fn check_name(file: &File, path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    let actual = {
        use std::os::unix::ffi::OsStrExt;
        let name = rustix::fs::getpath(file).map_err(|e| Error::from(std::io::Error::from(e)))?;
        std::path::PathBuf::from(std::ffi::OsStr::from_bytes(name.as_bytes()))
    };
    #[cfg(target_os = "linux")]
    let actual = {
        use std::os::fd::AsRawFd;
        fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))?
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let actual = {
        let _ = file;
        let parent = path.parent().unwrap_or(Path::new("."));
        let mut actual = None;
        for entry in fs::read_dir(parent)? {
            let entry = entry?;
            if Some(entry.file_name().as_os_str()) == path.file_name() {
                actual = Some(entry.path());
                break;
            }
        }
        actual.ok_or_else(Error::noncanonical_filename)?
    };
    if path.file_name().is_some() && actual.file_name() != path.file_name() {
        return Err(Error::path_spelling_mismatch());
    }
    Ok(())
}
