//! Search-only traversal, with readable descriptors only for required barriers.
use super::{Result, native};
use rustix::fs::{self, Mode, OFlags};
use std::{fs::File, path::PathBuf};

pub(super) struct Directory {
    pub file: File,
    pub path: PathBuf,
}
const FLAGS: OFlags = OFlags::DIRECTORY
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
fn search_flags() -> OFlags {
    #[cfg(target_os = "macos")]
    let access = OFlags::from_bits_retain(libc::O_SEARCH.cast_unsigned());
    #[cfg(not(target_os = "macos"))]
    let access = OFlags::PATH;
    FLAGS | access
}
impl Directory {
    pub fn root() -> Result<Self> {
        Ok(Self {
            file: File::from(fs::open("/", search_flags(), Mode::empty()).map_err(native)?),
            path: "/".into(),
        })
    }
    pub fn child(&self, name: &std::ffi::OsStr) -> rustix::io::Result<Self> {
        Ok(Self {
            file: File::from(fs::openat(&self.file, name, search_flags(), Mode::empty())?),
            path: self.path.join(name),
        })
    }
    pub fn readable(&self) -> Result<File> {
        let file = File::from(
            fs::openat(&self.file, ".", FLAGS | OFlags::RDONLY, Mode::empty()).map_err(native)?,
        );
        super::ancestor(&fs::fstat(&file).map_err(native)?)?;
        Ok(file)
    }
    fn sync(&self) -> Result<()> {
        let file = self.readable()?;
        #[cfg(test)]
        crate::disk::publication_hooks::step(&self.path, atomic_fs::Step::BeforeDirectorySync)?;
        file.sync_all()?;
        Ok(())
    }
}
pub(super) fn barrier(leaf: &Directory, parent: Option<&Directory>) -> Result<()> {
    // A failed mkdir publication stops before another child can be created.
    // Therefore only the deepest existing prefix and its parent can need a
    // retry barrier; higher ancestors are untouched. Never suppress EROFS.
    if let Some(parent) = parent {
        parent.sync()?;
    }
    leaf.sync()
}
