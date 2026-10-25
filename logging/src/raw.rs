//! Soft retention for inherited native output; this is not sanitized telemetry.
//! The active inode is never renamed. Bytes written during a trim can be lost.
mod paths;
#[cfg(test)]
mod tests;
mod timer;
use paths::Directory;
use rustix::fs::{FlockOperation, OFlags};
use std::{fs::File, io, os::fd::AsFd, os::unix::fs::FileExt, path::Path};
pub use timer::Guard;

pub const TRIM_BYTES: u64 = 10 * 1024 * 1024;
pub const TAIL_BYTES: usize = 1024 * 1024;

/// Admitted without starting a thread. Call `start` only after environment loading.
pub struct Owner {
    directory: Directory,
    active: File,
    name: String,
    lock: File,
    _lifetime: Box<dyn Send>,
}
impl Owner {
    /// Foreground or unrelated stderr is not managed and creates no files.
    /// Another admitted owner leaves the shared append stream untouched.
    pub fn admit(
        path: &Path,
        inherited: &impl AsFd,
        lifetime: impl Send + 'static,
    ) -> io::Result<Option<Self>> {
        let inherited_meta = rustix::fs::fstat(inherited)?;
        if rustix::fs::FileType::from_raw_mode(inherited_meta.st_mode)
            != rustix::fs::FileType::RegularFile
        {
            return Ok(None);
        }
        let expected = match std::fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if !paths::matches(&expected, &inherited_meta) {
            return Ok(None);
        }
        if !rustix::fs::fcntl_getfl(inherited)?.contains(OFlags::APPEND) {
            return Err(paths::invalid());
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(paths::invalid)?;
        let directory = Directory::open(path.parent().ok_or_else(paths::invalid)?)?;
        let active = directory.file(name, OFlags::RDWR | OFlags::APPEND)?;
        if !paths::matches(&active.metadata()?, &inherited_meta) {
            return Err(paths::invalid());
        }
        let lock = directory.file(
            &format!(".{name}.raw-owner.lock"),
            OFlags::RDWR | OFlags::CREATE,
        )?;
        match rustix::fs::flock(&lock, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => (),
            Err(rustix::io::Errno::WOULDBLOCK) => return Ok(None),
            Err(error) => return Err(error.into()),
        }
        let mut owner = Self {
            directory,
            active,
            name: name.into(),
            lock,
            _lifetime: Box::new(lifetime),
        };
        owner.check()?;
        owner.trim()?;
        Ok(Some(owner))
    }
    fn check(&self) -> io::Result<()> {
        self.directory.check()?;
        self.directory.named(&self.name, &self.active)?;
        self.directory
            .named(&format!(".{}.raw-owner.lock", self.name), &self.lock)
    }
    /// Publish the tail before truncating. Publication failures retain all active bytes.
    pub fn trim(&mut self) -> io::Result<bool> {
        self.trim_with(&atomic_fs::DefaultBarriers)
    }
    fn trim_with(&mut self, barriers: &impl atomic_fs::Barriers) -> io::Result<bool> {
        self.check()?;
        let length = self.active.metadata()?.len();
        if length <= TRIM_BYTES {
            return Ok(false);
        }
        let mut tail = vec![0; TAIL_BYTES];
        self.active
            .read_exact_at(&mut tail, length - TAIL_BYTES as u64)?;
        let previous = format!("{}.prev", self.name);
        self.directory.optional(&previous)?;
        let checked = paths::Checked {
            owner: self,
            barriers,
            previous: &previous,
        };
        atomic_fs::Publisher::new(
            (atomic_fs::Options::new())
                .with_durability(atomic_fs::Durability::AtomicVisible)
                .with_prefix(".hive-tmp-"),
        )
        .with_barriers(&checked)
        .write(self.directory.path.join(&previous), &tail)
        .map_err(io::Error::other)?;
        self.check()?;
        self.active.set_len(0)?;
        Ok(true)
    }
    pub fn start(self) -> io::Result<Guard> {
        timer::start(self)
    }
}
