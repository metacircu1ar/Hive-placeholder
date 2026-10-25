use super::FileChange;
use crate::{Result, files, invalid};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Component, Path},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Snapshot {
    hash: String,
    bytes: u64,
    pub(super) mode: u32,
}
impl Snapshot {
    pub(super) fn validate(&self) -> Result<()> {
        if self.hash.len() != 64
            || !self
                .hash
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            || self.mode & !0o777 != 0
            || self.mode & 0o002 != 0
        {
            return Err(invalid("Invalid migration file evidence."));
        }
        Ok(())
    }
}

pub(super) fn relative(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || !path.components().all(|c| matches!(c, Component::Normal(_)))
        || path.components().any(|c| {
            c.as_os_str().eq_ignore_ascii_case("lock")
                || Path::new(c.as_os_str())
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("lock"))
        })
    {
        return Err(invalid(
            "Migration path escapes its scope or names an ownership lock.",
        ));
    }
    Ok(())
}
pub(super) fn directory(path: &Path, private: bool) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir()
        || meta.uid() != rustix::process::geteuid().as_raw()
        || meta.mode() & 0o002 != 0
        || (private && meta.mode() & 0o077 != 0)
    {
        return Err(invalid("Unsafe migration directory."));
    }
    Ok(())
}

pub(super) fn scoped(root: &Path, path: &Path) -> Result<std::path::PathBuf> {
    relative(path)?;
    let (device, _) = identity(root)?;
    let mut current = root.to_owned();
    let mut parts = path.components().peekable();
    while let Some(part) = parts.next() {
        current.push(part.as_os_str());
        if parts.peek().is_some()
            && let Some(meta) = files::optional_metadata(&current)?
        {
            directory(&current, false)?;
            if meta.dev() != device {
                return Err(invalid("Migration path crosses a filesystem."));
            }
        }
    }
    Ok(current)
}

fn parents(root: &Path, path: &Path) -> Result<()> {
    scoped(root, path)?;
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Missing migration parent."))?;
    let mut current = root.to_owned();
    for part in parent.components() {
        current.push(part);
        if files::optional_metadata(&current)?.is_none() {
            files::directory(&current)?;
        }
        directory(&current, false)?;
    }
    scoped(root, path)?;
    Ok(())
}
pub(super) fn identity(path: &Path) -> Result<(u64, u64)> {
    directory(path, false)?;
    let metadata = fs::symlink_metadata(path)?;
    Ok((metadata.dev(), metadata.ino()))
}
fn open(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.uid() != rustix::process::geteuid().as_raw()
        || meta.mode() & 0o002 != 0
    {
        return Err(invalid("Unsafe migration file."));
    }
    let named = fs::symlink_metadata(path)?;
    if (meta.dev(), meta.ino()) != (named.dev(), named.ino()) {
        return Err(invalid("Migration file changed while opening."));
    }
    Ok(file)
}
pub(super) fn snapshot(path: &Path) -> Result<Option<Snapshot>> {
    if files::optional_metadata(path)?.is_none() {
        return Ok(None);
    }
    let mut file = open(path)?;
    let metadata = file.metadata()?;
    let mut hash = Sha256::new();
    let bytes = io::copy(&mut file, &mut hash)?;
    Ok(Some(Snapshot {
        hash: format!("{:x}", hash.finalize()),
        bytes,
        mode: metadata.mode() & 0o777,
    }))
}
pub(super) fn copy(source: &Path, target: &Path, expected: Option<&Snapshot>) -> Result<()> {
    if snapshot(source)?.as_ref() != expected {
        return Err(invalid("Migration input changed."));
    }
    let Some(expected) = expected else {
        return Ok(());
    };
    files::directory(
        target
            .parent()
            .ok_or_else(|| invalid("Missing migration parent."))?,
    )?;
    let mut reader = open(source)?;
    atomic_fs::Publisher::new((crate::names::PUBLICATION_OPTIONS).with_mode(expected.mode))
        .with_barriers(&atomic_fs::DefaultBarriers)
        .write_stream(target, |writer| io::copy(&mut reader, writer).map(|_| ()))
        .map_err(io::Error::other)?;
    if snapshot(target)?.as_ref() != Some(expected) || snapshot(source)?.as_ref() != Some(expected)
    {
        return Err(invalid("Migration copy changed."));
    }
    Ok(())
}
pub(super) fn record<T: DeserializeOwned>(path: &Path) -> Result<T> {
    // A migration can name thousands of pages. This bounded manifest has a
    // larger limit than individual setup records, identical on write and read.
    let file = open(path)?;
    let metadata = file.metadata()?;
    if metadata.mode() & 0o077 != 0 || metadata.len() > RECORD_BYTES {
        return Err(invalid("Unsafe or oversized migration record."));
    }
    let mut bytes = Vec::new();
    file.take(RECORD_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > RECORD_BYTES {
        return Err(invalid("Oversized migration record."));
    }
    if bytes.iter().find(|b| !b.is_ascii_whitespace()) != Some(&b'{') {
        return Err(invalid("Migration record must be an object."));
    }
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}
pub(super) fn create_record(
    path: &Path,
    value: &impl Serialize,
    barriers: &impl atomic_fs::Barriers,
) -> Result<()> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    if bytes.len() as u64 > RECORD_BYTES {
        return Err(invalid(
            "Migration record exceeds its recoverable size limit.",
        ));
    }
    atomic_fs::PreparedCreate::new(path, crate::names::PUBLICATION_OPTIONS)
        .and_then(|p| p.publish_with(&bytes, barriers))
        .map_err(io::Error::other)
}
const RECORD_BYTES: u64 = 64 * 1024 * 1024;
pub(super) fn sync(path: &Path, barriers: &impl atomic_fs::Barriers) -> Result<()> {
    if files::optional_metadata(path)?.is_some() {
        barriers.sync_file(&open(path)?)?;
    }
    // An optional absent destination may also have absent parent directories.
    // Persist its absence at the closest existing ancestor without creating them.
    let mut parent = path.parent();
    while let Some(candidate) = parent {
        if files::optional_metadata(candidate)?.is_some() {
            return sync_directory(candidate, barriers);
        }
        parent = candidate.parent();
    }
    Err(invalid("Missing migration parent."))
}

pub(super) fn sync_directory(path: &Path, barriers: &impl atomic_fs::Barriers) -> Result<()> {
    let expected = identity(path)?;
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(path)?;
    let held = directory.metadata()?;
    if (held.dev(), held.ino()) != expected {
        return Err(invalid("Migration directory changed while opening."));
    }
    barriers.observe(atomic_fs::Step::BeforeDirectorySync)?;
    barriers.sync_directory(&directory)?;
    barriers.observe(atomic_fs::Step::AfterDirectorySync)?;
    if identity(path)? != expected {
        return Err(invalid(
            "Migration directory changed during synchronization.",
        ));
    }
    Ok(())
}
pub(super) fn name(from: u32, to: u32) -> Result<String> {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    Ok(format!("{from}-{to}-{time}"))
}
pub(super) fn valid_name(name: &str, from: u32, to: u32) -> bool {
    name.strip_prefix(&format!("{from}-{to}-"))
        .is_some_and(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()))
}

pub(super) fn publish(
    root: &Path,
    staged: &Path,
    change: &FileChange,
    barriers: &impl atomic_fs::Barriers,
    owner: &dyn Fn() -> Result<()>,
) -> Result<()> {
    let target = scoped(root, &change.path)?;
    let current = snapshot(&target)?;
    if current == change.after {
        return sync(&target, barriers);
    }
    if current != change.before {
        return Err(invalid("External edit fences migration publication."));
    }
    let check = || {
        owner()?;
        scoped(root, &change.path)?;
        if snapshot(&target)? != change.before {
            return Err(invalid("External edit fences migration publication."));
        }
        Ok(())
    };
    let checked = Checked {
        barriers,
        check: &check,
    };
    if let Some(expected) = &change.after {
        parents(root, &change.path)?;
        let mut source = open(&scoped(staged, &change.path)?)?;
        let options = (crate::names::PUBLICATION_OPTIONS).with_mode(expected.mode);
        if change.before.is_none() {
            atomic_fs::PreparedCreate::new(&target, options)
                .and_then(|prepared| {
                    prepared.publish_stream(
                        |writer| io::copy(&mut source, writer).map(|_| ()),
                        &checked,
                    )
                })
                .map_err(io::Error::other)?;
        } else {
            atomic_fs::Publisher::new(options)
                .with_barriers(&checked)
                .write_stream(&target, |writer| io::copy(&mut source, writer).map(|_| ()))
                .map_err(io::Error::other)?;
        }
    } else {
        barriers.observe(atomic_fs::Step::BeforeRemove)?;
        check()?;
        fs::remove_file(&target)?;
        barriers.observe(atomic_fs::Step::AfterRemove)?;
        sync(&target, barriers)?;
    }
    if snapshot(&target)? != change.after {
        return Err(invalid("Published migration file changed."));
    }
    Ok(())
}
pub(super) struct Checked<'a, B> {
    pub(super) barriers: &'a B,
    pub(super) check: &'a dyn Fn() -> Result<()>,
}
impl<B: atomic_fs::Barriers> atomic_fs::Barriers for Checked<'_, B> {
    fn sync_file(&self, file: &File) -> Result<()> {
        self.barriers.sync_file(file)
    }
    fn sync_directory(&self, file: &File) -> Result<()> {
        self.barriers.sync_directory(file)
    }
    fn commit(&self, file: &File) -> Result<()> {
        self.barriers.commit(file)
    }
    fn observe(&self, step: atomic_fs::Step) -> Result<()> {
        self.barriers.observe(step)?;
        if step == atomic_fs::Step::BeforePublish {
            (self.check)()?;
        }
        Ok(())
    }
}
