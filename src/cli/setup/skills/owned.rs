//! A pending hash set lets an interrupted update safely resume without claiming user edits.
use super::{LIMIT, Result, files, invalid, io, publication, read_owned, recovery, safe};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::OpenOptions,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};
const RECORD: &str = ".hive-owned.json";
type Hashes = BTreeMap<PathBuf, String>;
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Record {
    files: Hashes,
    pending: Option<Hashes>,
}
pub(super) struct Group {
    root: PathBuf,
    files: BTreeMap<PathBuf, Vec<u8>>,
}
pub(super) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn hashes(files: &BTreeMap<PathBuf, Vec<u8>>) -> Hashes {
    files.iter().map(|(p, b)| (p.clone(), digest(b))).collect()
}
fn valid(path: &Path) -> bool {
    (path == Path::new("SKILL.md") || path.starts_with("bundle"))
        && path
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}
fn read(root: &Path) -> Result<Record> {
    safe(&root.join(RECORD))?;
    let Some(bytes) = files::read(&root.join(RECORD)).map_err(io)? else {
        return Ok(Record::default());
    };
    let record: Record = serde_json::from_slice(&bytes)
        .map_err(|cause| invalid("Invalid skill ownership.").with_source(cause))?;
    validate(&record)?;
    Ok(record)
}
fn validate(record: &Record) -> Result<()> {
    for entries in std::iter::once(&record.files).chain(record.pending.iter()) {
        if entries.len() > 2048
            || entries.iter().any(|(p, h)| {
                !valid(p) || h.len() != 64 || !h.bytes().all(|c| c.is_ascii_hexdigit())
            })
        {
            return Err(invalid(
                "Skill ownership paths or hashes exceed their limits; existing recovery \
                    evidence was preserved.",
            ));
        }
    }
    Ok(())
}
fn encode(record: &Record) -> Result<Vec<u8>> {
    validate(record)?;
    let bytes = files::encode(record).map_err(io)?;
    if bytes.len() as u64 > files::MAX_RECORD {
        return Err(invalid(
            "Skill ownership update exceeds 256 KiB; existing recovery evidence was preserved.",
        ));
    }
    Ok(bytes)
}
pub(super) fn present(root: &Path) -> Result<bool> {
    safe(&root.join(RECORD))?;
    Ok(files::read(&root.join(RECORD)).map_err(io)?.is_some())
}
pub(super) fn stale(root: &Path, desired: &BTreeMap<PathBuf, Vec<u8>>) -> Result<bool> {
    let record = read(root)?;
    if record.pending.is_some() || record.files != hashes(desired) {
        return Ok(true);
    }
    for (path, hash) in &record.files {
        safe(&root.join(path))?;
        if read_owned(&root.join(path))?
            .as_deref()
            .map(digest)
            .as_ref()
            != Some(hash)
        {
            return Ok(true);
        }
    }
    Ok(false)
}
impl Group {
    pub fn inspect(root: PathBuf, files: BTreeMap<PathBuf, Vec<u8>>) -> Result<Self> {
        if files.len() > 2048
            || files
                .iter()
                .any(|(path, bytes)| !valid(path) || bytes.len() as u64 > LIMIT)
        {
            return Err(invalid("Invalid skill paths, file count or file size."));
        }
        let group = Self { root, files };
        group.check(&read(&group.root)?)?;
        Ok(group)
    }
    pub fn paths(&self) -> Vec<PathBuf> {
        self.files.keys().map(|p| self.root.join(p)).collect()
    }
    fn paths_union(&self, record: &Record) -> BTreeSet<PathBuf> {
        self.files
            .keys()
            .chain(record.files.keys())
            .chain(record.pending.iter().flat_map(|p| p.keys()))
            .cloned()
            .collect()
    }
    fn check(&self, record: &Record) -> Result<Hashes> {
        let mut authorized = Hashes::new();
        for path in self.paths_union(record) {
            let full = self.root.join(&path);
            safe(&full)?;
            let Some(current) = read_owned(&full)? else {
                continue;
            };
            let hash = digest(&current);
            let owned = record.files.get(&path) == Some(&hash)
                || record.pending.as_ref().and_then(|p| p.get(&path)) == Some(&hash);
            // Adopt an identical unowned file, but never overwrite differing content.
            if !owned && self.files.get(&path) != Some(&current) {
                return Err(invalid(format!(
                    "User skill file differs: {}. Move it aside or omit --user-skills.",
                    full.display()
                )));
            }
            authorized.insert(path, hash);
        }
        Ok(authorized)
    }
    pub fn apply(&self) -> Result<bool> {
        let parent = self
            .root
            .parent()
            .ok_or_else(|| invalid("Skill needs a parent."))?;
        safe(parent)?;
        files::directory(parent).map_err(io)?;
        let name = self
            .root
            .file_name()
            .ok_or_else(|| invalid("Skill needs a name."))?
            .to_string_lossy();
        let path = parent.join(format!(".hive-{name}.lock"));
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
            .map_err(io)?;
        files::read(&path).map_err(io)?;
        let _guard = hive_setup_support::locks::Guard::exclusive(lock).map_err(|cause| {
            invalid("Another setup owns these skill copies.").with_source(cause)
        })?;
        self.apply_locked()
    }
    fn apply_locked(&self) -> Result<bool> {
        let record = read(&self.root)?;
        let current = self.check(&record)?;
        #[cfg(test)]
        AFTER_CHECK.with(|hook| {
            if let Some(hook) = hook.borrow_mut().take() {
                hook();
            }
        });
        let wanted = hashes(&self.files);
        let paths = self.paths_union(&record);
        let unchanged = record.pending.is_none() && record.files == wanted && current == wanted;
        // A retry must not enlarge a valid intent with the union of partially
        // installed old/new hashes. Keep authorization reads only in memory.
        let intent = if unchanged || record.pending.as_ref() == Some(&wanted) {
            record.clone()
        } else {
            Record {
                files: current.clone(),
                pending: Some(wanted.clone()),
            }
        };
        // Reject an unrepresentable retarget before any publication or recovery
        // effects; the old bounded record remains usable by the original update.
        let intent_bytes = encode(&intent)?;
        let final_bytes = encode(&Record {
            files: wanted,
            pending: None,
        })?;
        // Retire all old durability obligations BEFORE the next intent can omit
        // absent paths. A failed retry leaves the old recovery record untouched.
        for path in record
            .files
            .keys()
            .chain(record.pending.iter().flat_map(|p| p.keys()))
            .collect::<BTreeSet<_>>()
        {
            recovery::resync(&self.root.join(path), current.get(path).map(String::as_str))?;
        }
        // Also repeats file/parent barriers when resuming the same intent.
        write_record(&self.root, &intent_bytes)?;
        if unchanged {
            return Ok(false);
        }
        for path in paths {
            self.update(&path, &current)?;
        }
        write_record(&self.root, &final_bytes)?;
        Ok(true)
    }
    fn update(&self, path: &Path, authorized: &Hashes) -> Result<()> {
        let full = self.root.join(path);
        safe(&full)?;
        let current = read_owned(&full)?;
        let wanted = self.files.get(path);
        if current.as_ref() == wanted {
            return recovery::resync(&full, current.as_deref().map(digest).as_deref());
        }
        if let Some(bytes) = &current
            && authorized.get(path) != Some(&digest(bytes))
        {
            return Err(invalid(format!(
                "Skill changed during update: {}",
                full.display()
            )));
        }
        publication::update(&full, current.as_deref(), wanted.map(Vec::as_slice))?;
        Ok(())
    }
}

fn write_record(root: &Path, bytes: &[u8]) -> Result<()> {
    let path = root.join(RECORD);
    publication::checkpoint(
        &path,
        hive_setup_support::checkpoints::Checkpoint::BeforeIntent,
    )
    .map_err(io)?;
    files::write(&path, bytes, true).map_err(io)?;
    publication::checkpoint(
        &path,
        hive_setup_support::checkpoints::Checkpoint::AfterIntent,
    )
    .map_err(|source| publication::error(atomic_fs::Error::new(source, true)))
}

#[cfg(test)]
thread_local! {
    static AFTER_CHECK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
#[test]
#[allow(clippy::unwrap_used)]
fn edit_after_validation_is_never_adopted_and_overwritten() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("skill");
    let old = BTreeMap::from([(PathBuf::from("SKILL.md"), b"owned old".to_vec())]);
    Group::inspect(root.clone(), old).unwrap().apply().unwrap();
    let new = BTreeMap::from([(PathBuf::from("SKILL.md"), b"owned new".to_vec())]);
    let update = Group::inspect(root.clone(), new.clone()).unwrap();
    let guide = root.join("SKILL.md");
    let edit = guide.clone();
    AFTER_CHECK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            std::fs::write(edit, "concurrent user edit").unwrap();
        }));
    });
    assert!(update.apply().is_err());
    assert_eq!(
        std::fs::read_to_string(&guide).unwrap(),
        "concurrent user edit"
    );
    assert!(Group::inspect(root.clone(), new.clone()).is_err());
    assert_eq!(
        read(&root).unwrap().files[Path::new("SKILL.md")],
        digest(b"owned old")
    );
    // Restoring the pre-edit owned bytes allows the pending update to finish.
    std::fs::write(&guide, "owned old").unwrap();
    Group::inspect(root, new).unwrap().apply().unwrap();
    assert_eq!(std::fs::read_to_string(guide).unwrap(), "owned new");
}

#[cfg(test)]
mod tests;
