use super::{File, Path, invalid, io};
use rustix::fs::{Dir, Mode, OFlags, open, openat};
use std::os::unix::fs::MetadataExt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Identity {
    device: u64,
    inode: u64,
}
pub(super) fn identity(file: &File) -> io::Result<Identity> {
    let meta = file.metadata()?;
    Ok(Identity {
        device: meta.dev(),
        inode: meta.ino(),
    })
}
pub(super) fn directory(path: &Path) -> io::Result<File> {
    let file = File::from(open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    check(&file, true)?;
    Ok(file)
}
fn check(file: &File, directory: bool) -> io::Result<()> {
    let meta = file.metadata()?;
    if !directory && meta.nlink() == 0 {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Log segment was pruned",
        ));
    }
    if meta.uid() != rustix::process::geteuid().as_raw()
        || meta.mode() & 0o077 != 0
        || if directory {
            !meta.is_dir()
        } else {
            !meta.is_file() || meta.nlink() != 1
        }
    {
        return Err(invalid(
            "Log entries must be private and owned by this user",
        ));
    }
    Ok(())
}
pub(super) struct Segment {
    pub file: File,
    pub identity: Identity,
    pub length: u64,
}
pub(super) fn segments(directory: &File, name: &str) -> io::Result<Vec<Segment>> {
    let before = names(directory, name)?;
    let segments = before
        .iter()
        .map(|(_, name)| {
            let file = File::from(openat(
                directory,
                name.as_c_str(),
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )?);
            check(&file, false)?;
            Ok(Segment {
                identity: identity(&file)?,
                length: file.metadata()?.len(),
                file,
            })
        })
        .collect::<io::Result<Vec<_>>>()?;
    if before != names(directory, name)? {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "Log rotation changed the snapshot",
        ));
    }
    Ok(segments)
}
fn names(directory: &File, name: &str) -> io::Result<Vec<(u128, std::ffi::CString)>> {
    let prefix = format!(".{name}.");
    let mut names = Vec::new();
    for entry in Dir::read_from(directory)? {
        let entry = entry?;
        let Some(text) = entry.file_name().to_str().ok() else {
            continue;
        };
        let order = if text == name {
            u128::MAX
        } else {
            let Some(suffix) = text.strip_prefix(&prefix) else {
                continue;
            };
            let Some(number) = suffix
                .strip_suffix(".log")
                .or_else(|| suffix.strip_suffix(".torn"))
            else {
                continue;
            };
            let number = number
                .parse::<u128>()
                .map_err(|_| invalid("Invalid log segment number"))?;
            if number == u128::MAX {
                return Err(invalid("Invalid log segment number"));
            }
            number
        };
        names.push((order, entry.file_name().to_owned()));
        if names.len() > 65 {
            return Err(invalid("Too many retained log segments"));
        }
    }
    names.sort_by_key(|(order, _)| *order);
    Ok(names)
}
