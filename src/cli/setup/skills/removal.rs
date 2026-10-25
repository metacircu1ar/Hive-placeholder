//! The durable group intent owns one unlink; absent retries still sync its parent.
use super::{
    Result, invalid,
    publication::{Publication, io},
    read_file, recovery,
};
use atomic_fs::{Barriers, Step};
use std::{
    fs::{File, OpenOptions},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

pub(super) fn unlink(path: &Path, barriers: &Publication<'_>) -> Result<File> {
    let (file, bytes) =
        read_file(path)?.ok_or_else(|| invalid("Skill disappeared before removal."))?;
    if Some(bytes.as_slice()) != barriers.current {
        return Err(invalid("Skill changed before removal."));
    }
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Skill needs a parent."))?;
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_NONBLOCK)
        .open(parent)
        .map_err(io)?;
    recovery::same(&directory, parent).map_err(io)?;
    recovery::same(&file, path).map_err(io)?;
    barriers.observe(Step::BeforeRemove).map_err(io)?;
    recovery::same(&directory, parent).map_err(io)?;
    recovery::same(&file, path).map_err(io)?;
    let name = path
        .file_name()
        .ok_or_else(|| invalid("Skill needs a file name."))?;
    // One atomic namespace removal. This is not CAS against an arbitrary editor;
    // the operation lock excludes other setup writers, and observed swaps refuse.
    rustix::fs::unlinkat(&directory, name, rustix::fs::AtFlags::empty())
        .map_err(|cause| io(cause.into()))?;
    // The caller must retain this exact directory through the unlink barrier.
    Ok(directory)
}
