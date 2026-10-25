//! New services must not appear behind an installer's completed inventory.
use crate::{Result, files, invalid};
use std::{fs::OpenOptions, os::unix::fs::OpenOptionsExt, path::Path};

pub(super) fn guard(program: &Path) -> Result<Option<crate::locks::Guard>> {
    let bin = program
        .parent()
        .ok_or_else(|| invalid("Program needs a parent."))?;
    let prefix = bin
        .parent()
        .ok_or_else(|| invalid("Program needs a prefix."))?;
    if bin.file_name().is_some_and(|name| name == "bin") {
        if prefix
            .parent()
            .is_some_and(|p| p.ends_with("hive/generations"))
        {
            return Err(invalid(
                "Use the stable PREFIX/bin command for a managed service, not a generation image.",
            ));
        }
        let path = prefix.join("lib/.hive-install.lock");
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        files::read(&path)?;
        let guard = crate::locks::Guard::exclusive(file)
            .map_err(|error| crate::context("Cannot acquire service registration lock", error))?;
        // The spec may have been prepared before removal acquired the lock.
        files::executable(program)?;
        return Ok(Some(guard));
    }
    Ok(None)
}
