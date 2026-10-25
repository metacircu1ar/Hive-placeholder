//! A bounded observation of one file; editors still own their write atomicity.
use super::{Error, Result, check_file, check_name};
use std::{
    fs::{self, Metadata, OpenOptions},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

pub(super) fn read(path: &Path, max: usize) -> Result<Vec<u8>> {
    for _ in 0..3 {
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(Error::opening_file)?;
        let before = file.metadata()?;
        check_file(&before)?;
        check_name(&file, path)?;
        let bytes = crate::bounded::bytes(&mut file, max, Error::file_size_limit);
        if bytes
            .as_ref()
            .is_err_and(|error| error.code != crate::ErrorCode::SizeLimit)
        {
            return bytes;
        }
        #[cfg(test)]
        step(path)?;
        let after = file.metadata()?;
        let named = match fs::symlink_metadata(path) {
            Ok(named) => named,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        check_file(&named)?;
        // Atomic replacement legitimately detaches the already-checked inode.
        // Reopen the new name; a hard-linked or unsafe named file still refuses.
        if after.nlink() == 0 {
            continue;
        }
        check_file(&after)?;
        if unchanged(&before, &after) && unchanged(&after, &named) {
            return bytes;
        }
    }
    Err(Error::file_changed_during_read())
}

fn unchanged(a: &Metadata, b: &Metadata) -> bool {
    a.dev() == b.dev()
        && a.ino() == b.ino()
        && a.mode() == b.mode()
        && a.nlink() == b.nlink()
        && a.uid() == b.uid()
        && a.gid() == b.gid()
        && a.len() == b.len()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
        && a.ctime() == b.ctime()
        && a.ctime_nsec() == b.ctime_nsec()
}

#[cfg(test)]
pub(super) use hooks::{step, with};

#[cfg(test)]
mod hooks {
    use std::{cell::RefCell, io, path::Path};
    type Hook = Box<dyn Fn(&Path) -> io::Result<()>>;
    thread_local! { static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) }; }
    struct Restore(Option<Hook>);
    impl Drop for Restore {
        fn drop(&mut self) {
            HOOK.replace(self.0.take());
        }
    }
    pub(in crate::fs) fn with<T>(
        hook: impl Fn(&Path) -> io::Result<()> + 'static,
        run: impl FnOnce() -> T,
    ) -> T {
        let _restore = Restore(HOOK.replace(Some(Box::new(hook))));
        run()
    }
    pub(in crate::fs) fn step(path: &Path) -> io::Result<()> {
        HOOK.with_borrow(|hook| hook.as_ref().map_or(Ok(()), |hook| hook(path)))
    }
}
