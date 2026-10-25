//! Resolve bounded key bytes without making them printable or serializable.
use crate::{
    config::Profile,
    error::{Error, Result},
    os,
};
use rustix::fs::{self, Mode, OFlags};
use std::{
    fs::File,
    os::unix::fs::MetadataExt,
    path::{Component, Path},
};

const KEY_BYTES: usize = 16 * 1024;
pub(crate) struct Secret(pub(super) String);

impl Secret {
    fn validate(self) -> Result<Self> {
        if self.0.is_empty() {
            return Err(missing());
        }
        if self.0.len() > KEY_BYTES || self.0.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(invalid());
        }
        Ok(self)
    }
}
fn missing() -> Error {
    Error::authentication_required("The configured API key source is missing or empty.")
}
fn invalid() -> Error {
    Error::config_invalid(
        "The API key must be bounded UTF-8 text without whitespace or control characters.",
    )
}
fn unsafe_source() -> Error {
    Error::unsafe_state(
        "API key files must be private regular files owned by this user under \
            trusted directories, without symlinks or hard links.",
    )
}
fn source_io(error: std::io::Error) -> Error {
    if error.kind() == std::io::ErrorKind::NotFound {
        missing().with_source(error)
    } else {
        Error::state_io_from(error)
    }
}
pub(super) fn resolve(profile: &Profile) -> Result<Secret> {
    profile.validate()?;
    match (&profile.api_key_env, &profile.api_key_file) {
        (Some(name), None) => {
            let value = std::env::var_os(name).ok_or_else(missing)?;
            let text = value.to_str().ok_or_else(invalid)?;
            Secret(text.into()).validate()
        }
        (None, Some(path)) => read_file(path),
        _ => Err(Error::config_invalid(
            "An API key needs exactly one configured source.",
        )),
    }
}
fn read_file(path: &Path) -> Result<Secret> {
    let file = open_file(path)?;
    let before = file.metadata().map_err(source_io)?;
    if !before.is_file()
        || before.uid() != os::effective_uid()
        || before.mode() & 0o7177 != 0
        || before.nlink() != 1
    {
        return Err(unsafe_source());
    }
    let bytes = os::read_capped(&file, (KEY_BYTES + 3) as u64).map_err(source_io)?;
    let after = file.metadata().map_err(source_io)?;
    if (
        before.len(),
        before.mtime(),
        before.mtime_nsec(),
        before.ctime(),
        before.ctime_nsec(),
    ) != (
        after.len(),
        after.mtime(),
        after.mtime_nsec(),
        after.ctime(),
        after.ctime_nsec(),
    ) {
        return Err(Error::agent_busy(
            "API key file changed while reading; replace it atomically and retry.",
        ));
    }
    let text = std::str::from_utf8(&bytes).map_err(|error| invalid().with_source(error))?;
    let text = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text);
    Secret(text.into()).validate()
}
fn open_file(path: &Path) -> Result<File> {
    if !path.is_absolute() {
        return Err(Error::invalid_path(
            "API key file must be absolute after configuration resolution.",
        ));
    }
    // Ancestors need search permission, not directory-listing permission.
    #[cfg(target_os = "macos")]
    let access = OFlags::from_bits_retain(libc::O_SEARCH.cast_unsigned());
    #[cfg(not(target_os = "macos"))]
    let access = OFlags::PATH;
    let flags = access | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::DIRECTORY;
    let mut parent =
        fs::open("/", flags, Mode::empty()).map_err(|error| source_io(error.into()))?;
    let mut parts = path.components().peekable();
    while let Some(part) = parts.next() {
        let Component::Normal(name) = part else {
            if matches!(part, Component::RootDir | Component::CurDir) {
                continue;
            }
            return Err(unsafe_source());
        };
        let metadata = fs::fstat(&parent).map_err(|error| source_io(error.into()))?;
        if !hive_setup_support::directories::ancestor(
            metadata.st_uid,
            metadata.st_mode,
            os::effective_uid(),
        ) {
            return Err(unsafe_source());
        }
        if parts.peek().is_none() {
            let flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK;
            return fs::openat(&parent, name, flags, Mode::empty())
                .map(File::from)
                .map_err(|error| source_io(error.into()));
        }
        parent = fs::openat(&parent, name, flags, Mode::empty())
            .map_err(|error| source_io(error.into()))?;
    }
    Err(unsafe_source())
}

#[cfg(test)]
mod tests;
