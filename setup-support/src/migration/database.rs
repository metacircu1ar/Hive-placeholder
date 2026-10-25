//! SQLite inspection and VACUUM operate on private copies, never on the original
//! database or its SHM. The product supplies SQLite and its exact schema reader.
//! Caller-owned root exclusion must cover every original SQLite connection.
use super::storage::{self, Snapshot};
use crate::{Result, files, invalid};
use std::{
    fs, io,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

pub struct Copy {
    source: PathBuf,
    originals: Vec<Option<Snapshot>>,
    scratch: tempfile::TempDir,
}

const SUFFIXES: &[&str] = &["", "-wal", "-journal", "-shm"];

impl Copy {
    pub fn open(source: &Path) -> Result<Self> {
        let source = files::absolute(source)?;
        let originals = SUFFIXES
            .iter()
            .map(|s| storage::snapshot(&companion(&source, s)))
            .collect::<Result<Vec<_>>>()?;
        if originals[0].is_none() {
            return Err(invalid("Migration database is missing."));
        }
        let scratch = tempfile::Builder::new()
            .prefix("hive-database-inspection-")
            .permissions(fs::Permissions::from_mode(0o700))
            // SQLite NOFOLLOW rejects symlink ancestors as well as the leaf.
            // macOS commonly exposes its temporary directory through /var.
            .tempdir_in(fs::canonicalize(std::env::temp_dir())?)?;
        let copy = Self {
            source,
            originals,
            scratch,
        };
        for (suffix, expected) in SUFFIXES.iter().zip(&copy.originals) {
            storage::copy(
                &companion(&copy.source, suffix),
                &companion(&copy.path(), suffix),
                expected.as_ref(),
            )?;
        }
        copy.check_source()?;
        Ok(copy)
    }

    pub fn path(&self) -> PathBuf {
        self.scratch.path().join("database.sqlite")
    }

    pub fn check_source(&self) -> Result<()> {
        for (suffix, expected) in SUFFIXES.iter().zip(&self.originals) {
            if &storage::snapshot(&companion(&self.source, suffix))? != expected {
                return Err(invalid(
                    "Original database changed during migration inspection.",
                ));
            }
        }
        Ok(())
    }

    /// The callback validates the copied source, runs VACUUM INTO with a bound
    /// destination, and checks the resulting schema/data before returning.
    /// Publication is no-replace and durable; existing backups are never reused.
    pub fn vacuum(
        &self,
        destination: &Path,
        owner: &dyn Fn() -> Result<()>,
        produce: impl FnOnce(&Path, &Path) -> Result<()>,
    ) -> Result<()> {
        owner()?;
        self.check_source()?;
        let output = self.scratch.path().join("vacuum.sqlite");
        produce(&self.path(), &output)?;
        let expected = storage::snapshot(&output)?
            .ok_or_else(|| invalid("VACUUM did not produce a database."))?;
        let mut source = fs::File::open(&output)?;
        let check = || {
            owner()?;
            self.check_source()
        };
        check()?;
        let checked = storage::Checked {
            barriers: &atomic_fs::DefaultBarriers,
            check: &check,
        };
        atomic_fs::PreparedCreate::new(destination, crate::names::PUBLICATION_OPTIONS)
            .and_then(|prepared| {
                prepared
                    .publish_stream(|writer| io::copy(&mut source, writer).map(|_| ()), &checked)
            })
            .map_err(io::Error::other)?;
        // VACUUM may use the process umask; backup publication always uses 0600.
        let mut saved = storage::snapshot(destination)?
            .ok_or_else(|| invalid("Database backup is missing."))?;
        saved.mode = expected.mode;
        if saved != expected {
            return Err(invalid(
                "Published database backup differs from VACUUM output.",
            ));
        }
        self.check_source()
    }
}

fn companion(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    name.into()
}
