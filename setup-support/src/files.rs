//! Bounded private records; never follow a managed file's symlink.
use crate::{Result, invalid};
mod publication;
pub use publication::write;
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

pub const MAX_RECORD: u64 = 256 * 1024;

/// Observe an entry without following its leaf; only ENOENT means absence.
pub fn optional_metadata(path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn absolute(path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(invalid("Paths must not contain '..'."));
    }
    if optional_metadata(&path)?.is_some_and(|m| m.file_type().is_symlink()) {
        return Err(invalid("Managed path is a symlink."));
    }
    location(&path)
}

/// Canonical scope/base identity, including a user-owned symlink at its leaf.
pub fn location(path: &Path) -> Result<PathBuf> {
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(invalid("Paths must not contain '..'."));
    }
    let path = std::path::absolute(path)?;
    let mut ancestor = path.as_path();
    let mut missing = Vec::new();
    while !ancestor.try_exists()? {
        // A dangling link is not an absent component.
        if optional_metadata(ancestor)?.is_some() {
            return Err(invalid("Path contains a dangling symlink."));
        }
        missing.push(
            ancestor
                .file_name()
                .ok_or_else(|| invalid("Invalid path."))?,
        );
        ancestor = ancestor.parent().ok_or_else(|| invalid("Invalid path."))?;
    }
    let mut resolved = ancestor.canonicalize()?;
    for name in missing.into_iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

/// Resolve ancestors but retain the installed executable's stable leaf symlink.
pub fn executable(path: &Path) -> Result<PathBuf> {
    let path = if path.components().count() == 1 {
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|p| p.join(path))
            .find(|p| p.is_file())
            .ok_or_else(|| invalid(format!("Executable not found: {}", path.display())))?
    } else {
        path.to_owned()
    };
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Executable needs a directory."))?
        .canonicalize()?;
    let path = parent.join(
        path.file_name()
            .ok_or_else(|| invalid("Executable needs a name."))?,
    );
    let meta = fs::metadata(&path)?;
    if !meta.is_file() || meta.mode() & 0o111 == 0 {
        return Err(invalid("Program is not executable."));
    }
    Ok(path)
}

pub fn directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m)
            if m.is_dir()
                && crate::directories::owned(
                    m.uid(),
                    m.mode(),
                    rustix::process::getuid().as_raw(),
                ) =>
        {
            sync_ancestry(path)
        }
        Ok(_) => Err(invalid(format!(
            "Unsafe setup directory: {}",
            path.display()
        ))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                if parent.exists() {
                    let m = fs::metadata(parent)?;
                    if !m.is_dir()
                        || !crate::directories::ancestor(
                            m.uid(),
                            m.mode(),
                            rustix::process::getuid().as_raw(),
                        )
                    {
                        return Err(invalid("Setup directory has an untrusted parent."));
                    }
                } else {
                    directory(parent)?;
                }
            }
            let parent = path
                .parent()
                .ok_or_else(|| invalid("Directory needs a parent."))?;
            // Refuse an inaccessible/unsupported required barrier before mkdir;
            // otherwise a retry could mistake the leftover directory for an
            // already-published ancestor. Retain the descriptor through publication.
            let parent_file = File::open(parent)?;
            sync_directory(parent, &parent_file)?;
            match fs::DirBuilder::new().mode(0o700).create(path) {
                Ok(()) => (),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    return directory(path);
                }
                Err(error) => return Err(error),
            }
            sync_directory(parent, &parent_file)?;
            sync_ancestry(path)
        }
        Err(e) => Err(e),
    }
}

pub fn read(path: &Path) -> Result<Option<Vec<u8>>> {
    record(path)?.map(|file| record_bytes(&file)).transpose()
}

fn record(path: &Path) -> Result<Option<File>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let m = file.metadata()?;
    if !m.is_file()
        || m.uid() != rustix::process::getuid().as_raw()
        || m.mode() & 0o077 != 0
        || m.len() > MAX_RECORD
        || m.nlink() != 1
    {
        return Err(invalid(format!(
            "Expected a bounded owned mode-0600 file: {}",
            path.display()
        )));
    }
    Ok(Some(file))
}

fn record_bytes(file: &File) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(MAX_RECORD + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_RECORD {
        return Err(invalid("Setup record exceeds 256 KiB."));
    }
    Ok(bytes)
}

pub fn log(path: &Path) -> Result<()> {
    directory(
        path.parent()
            .ok_or_else(|| invalid("Log needs a parent."))?,
    )?;
    let file = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.uid() != rustix::process::getuid().as_raw()
        || meta.mode() & 0o077 != 0
        || meta.nlink() != 1
    {
        return Err(invalid(
            "Service log must be an owned private regular file.",
        ));
    }
    Ok(())
}

pub fn remove(path: &Path, expected: &[u8]) -> Result<()> {
    if read(path)?.as_deref() != Some(expected) {
        return Err(invalid("Owned record changed; refusing removal."));
    }
    fs::remove_file(path)?;
    sync_parent(path)
}
fn sync_ancestry(path: &Path) -> Result<()> {
    // A prior attempt may have created any ancestor before its barrier failed.
    for (depth, directory) in path.ancestors().enumerate() {
        match File::open(directory).and_then(|file| sync_directory(directory, &file)) {
            Ok(()) => (),
            // Each recursive creation flushes its own immediate parent before
            // creating anything below it. Higher ancestors therefore cannot
            // contain an unflushed entry created by this attempt or its retry.
            Err(error)
                if depth > 1
                    && (matches!(
                        error.kind(),
                        std::io::ErrorKind::PermissionDenied
                            | std::io::ErrorKind::ReadOnlyFilesystem
                            | std::io::ErrorKind::InvalidInput
                            | std::io::ErrorKind::Unsupported
                    ) || error
                        .raw_os_error()
                        .is_some_and(|code| [libc::ENOTSUP, libc::EOPNOTSUPP].contains(&code))) => {
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
fn sync_directory(path: &Path, file: &File) -> Result<()> {
    #[cfg(not(test))]
    let _ = path;
    #[cfg(test)]
    if let Some(errno) = FAIL_DIRECTORY_SYNC.with(|fault| {
        fault
            .borrow()
            .as_ref()
            .filter(|(candidate, _)| candidate == path)
            .map(|(_, errno)| *errno)
    }) {
        return Err(std::io::Error::from_raw_os_error(errno));
    }
    file.sync_all()
}
#[cfg(test)]
thread_local! {
    pub(crate) static FAIL_DIRECTORY_SYNC: std::cell::RefCell<Option<(PathBuf,
         i32)>> = const { std::cell::RefCell::new(None) };
    pub(crate) static FAIL_SYNC: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
    pub(crate) static SYNCED: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
}
pub fn sync_parent(path: &Path) -> Result<()> {
    #[cfg(test)]
    before_parent_sync(path)?;
    File::open(
        path.parent()
            .ok_or_else(|| invalid("Path needs a parent."))?,
    )?
    .sync_all()
}
#[cfg(test)]
fn before_parent_sync(path: &Path) -> Result<()> {
    {
        SYNCED.with(|paths| paths.borrow_mut().push(path.to_owned()));
        if FAIL_SYNC.with(|fault| fault.borrow().as_deref() == Some(path)) {
            return Err(std::io::Error::other("injected parent sync failure"));
        }
    }
    Ok(())
}

pub fn json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    read(path)?
        .map(|b| serde_json::from_slice(&b).map_err(invalid_json))
        .transpose()
}
pub fn encode<T: serde::Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec_pretty(value).map_err(invalid_json)
}
#[derive(Debug)]
struct RecordError(serde_json::Error);
impl std::fmt::Display for RecordError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Unsupported or malformed setup record.")
    }
}
impl std::error::Error for RecordError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}
fn invalid_json(cause: serde_json::Error) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, RecordError(cause))
}
