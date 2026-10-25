use super::*;
use crate::disk::System;

pub(super) struct DirectoryFault {
    pub root: PathBuf,
    pub leaf: &'static str,
    pub after: bool,
    pub crash: bool,
}
pub(super) const LEAVES: [&str; 3] = ["heads", "receipts", ""];
impl DirectoryFault {
    fn selected(&self, file: &File) -> io::Result<bool> {
        let metadata = file.metadata()?;
        for stage in preparation(&self.root) {
            // Exclude directory-creation syncs before the prepared tree is complete.
            if stage.join("identity.json").exists() {
                let named = fs::metadata(stage.join(self.leaf))?;
                if named.dev() == metadata.dev() && named.ino() == metadata.ino() {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    fn interrupt(&self) -> io::Result<()> {
        if self.crash {
            std::process::exit(73);
        }
        Err(io::Error::from_raw_os_error(libc::EIO))
    }
}
impl Durability for DirectoryFault {
    fn fsync(&self, file: &File) -> io::Result<()> {
        System.fsync(file)
    }
    fn ordering_barrier(&self, file: &File) -> io::Result<()> {
        System.ordering_barrier(file)
    }
    fn full_flush(&self, file: &File) -> io::Result<()> {
        System.full_flush(file)
    }
    fn sync_directory(&self, file: &File) -> io::Result<()> {
        let selected = self.selected(file)?;
        if selected && !self.after {
            self.interrupt()?;
        }
        System.sync_directory(file)?;
        if selected && self.after {
            self.interrupt()?;
        }
        Ok(())
    }
}

#[test]
fn every_prepublication_directory_sync_failure_retains_protected_complete_stage() {
    for leaf in LEAVES {
        for after in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let fault = Arc::new(DirectoryFault {
                root: root.path().to_owned(),
                leaf,
                after,
                crash: false,
            });
            let error = Disk::acquire_with(root.path(), true, fault).err().unwrap();
            assert!(!error.file_committed);
            assert!(!root.path().join(".comb").exists());
            let stages = preparation(root.path());
            assert_eq!(stages.len(), 1);
            protection(&stages[0]);
            let before = fs::read(stages[0].join("identity.json")).unwrap();
            drop(Disk::acquire(root.path(), true).unwrap());
            complete(root.path());
            assert_eq!(fs::read(stages[0].join("identity.json")).unwrap(), before);
        }
    }
}
