//! Durable, hash-checked publication of an explicit released-format step.
//! Product adapters supply exact readers, transformations and exclusive ownership;
//! this module never supplies a development migration or infers a file's format.
pub mod database;
mod preparation;
mod storage;
mod wire;
use crate::{Result, files, invalid};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
use storage::{Snapshot, copy, snapshot};

pub const PENDING: &str = "migration-intent.json";

/// Finish an uncertain intent deletion before permitting ordinary writes. The
/// caller holds root ownership throughout. Inspection/dry-run must not call this.
/// No history means no migration deletion to settle; fresh roots stay untouched.
pub fn admit_writes(
    admin: &Path,
    owner: &dyn Fn() -> Result<()>,
    barriers: &impl atomic_fs::Barriers,
) -> Result<()> {
    owner()?;
    let pending = admin.join(PENDING);
    if files::optional_metadata(&pending)?.is_some() {
        return Err(invalid("Recover the pending migration before writing."));
    }
    let history = admin.join("migrations");
    if files::optional_metadata(&history)?.is_none() {
        return Ok(());
    }
    storage::directory(&history, true)?;
    storage::directory(admin, true)?;
    storage::sync_directory(admin, barriers)?;
    owner()?;
    if files::optional_metadata(&pending)?.is_some() {
        return Err(invalid("Migration intent changed during admission."));
    }
    Ok(())
}

/// Paths are relative to the authoritative root. Include every affected file,
/// including absent destinations, and the marker. Never include ownership locks
/// or native provider homes. Validators must use exact released structures.
pub trait Step {
    fn from(&self) -> u32;
    fn to(&self) -> u32;
    fn files(&self, root: &Path) -> Result<Vec<PathBuf>>;
    fn validate_before(&self, root: &Path) -> Result<()>;
    fn transform(&self, staged: &Path) -> Result<()>;
    fn validate_after(&self, root: &Path) -> Result<()>;
    /// Every SQLite database being changed, also named in `files()` together with
    /// all three companions (including absent files). Native provider databases
    /// never belong in this list.
    fn databases(&self) -> Vec<PathBuf> {
        Vec::new()
    }
    /// Validate the private source with the frozen reader, VACUUM INTO the bound
    /// destination, and validate the result. Never opens the original database.
    fn vacuum(&self, _source: &Path, _destination: &Path) -> Result<()> {
        Err(invalid("The migration step has no SQLite backup adapter."))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct FileChange {
    path: PathBuf,
    #[serde(deserialize_with = "wire::optional_object")]
    before: Option<Snapshot>,
    #[serde(deserialize_with = "wire::optional_object")]
    after: Option<Snapshot>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub from: u32,
    pub to: u32,
    pub backup: String,
    stage: String,
    root_device: u64,
    root_inode: u64,
    #[serde(deserialize_with = "wire::objects")]
    files: Vec<FileChange>,
    #[serde(deserialize_with = "wire::objects")]
    databases: Vec<DatabaseBackup>,
}

#[derive(Debug, Serialize)]
pub struct Backup {
    pub name: String,
    pub completed: bool,
    pub pending: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct DatabaseBackup {
    path: PathBuf,
    #[serde(deserialize_with = "wire::object")]
    snapshot: Snapshot,
}

/// One product root and private administration directory, under caller-owned
/// exclusion. All adapter validation runs while that same owner remains alive.
pub struct Engine<'a> {
    root: &'a Path,
    admin: &'a Path,
    marker: &'a Path,
    owner: &'a dyn Fn() -> Result<()>,
}
impl<'a> Engine<'a> {
    pub fn new(
        root: &'a Path,
        admin: &'a Path,
        marker: &'a Path,
        owner: &'a dyn Fn() -> Result<()>,
    ) -> Result<Self> {
        storage::relative(marker)?;
        if files::absolute(root)? != root
            || files::absolute(admin)? != admin
            || !admin.starts_with(root)
        {
            return Err(invalid(
                "Migration paths must be canonical and remain inside the root.",
            ));
        }
        storage::directory(root, false)?;
        storage::directory(admin, true)?;
        owner()?;
        Ok(Self {
            root,
            admin,
            marker,
            owner,
        })
    }

    /// Zero-write inspection: never creates locks, backup directories or scratch.
    pub fn pending(&self) -> Result<Option<Receipt>> {
        (self.owner)()?;
        let path = self.admin.join(PENDING);
        if files::optional_metadata(&path)?.is_none() {
            return Ok(None);
        }
        let receipt: Receipt = storage::record(&path)?;
        self.check_receipt(&receipt)?;
        Ok(Some(receipt))
    }

    pub fn history(&self) -> Result<Vec<Receipt>> {
        (self.owner)()?;
        let base = self.admin.join("migrations");
        if files::optional_metadata(&base)?.is_none() {
            return Ok(Vec::new());
        }
        storage::directory(&base, true)?;
        let mut receipts = Vec::new();
        for entry in std::fs::read_dir(base)? {
            let entry = entry?;
            if entry.path().extension().is_some_and(|e| e == "json") {
                let receipt: Receipt = storage::record(&entry.path())?;
                self.check_receipt(&receipt)?;
                if entry.file_name() != format!("{}.json", receipt.stage).as_str() {
                    return Err(invalid(
                        "Migration receipt name disagrees with its identity.",
                    ));
                }
                receipts.push(receipt);
            }
        }
        receipts.sort_by(|a, b| a.backup.cmp(&b.backup));
        Ok(receipts)
    }

    /// Include durable backups whose transformation failed before intent. They
    /// remain visible even though no completed migration exists for them.
    pub fn backups(&self) -> Result<Vec<Backup>> {
        (self.owner)()?;
        let directory = self.admin.join("backups");
        if files::optional_metadata(&directory)?.is_none() {
            return Ok(Vec::new());
        }
        storage::directory(&directory, true)?;
        let history = self.history()?;
        let pending = self.pending()?;
        let mut backups = Vec::new();
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| invalid("Invalid backup name."))?;
            // Unpublished helper preparations are not durable named backups.
            if name.starts_with('.') {
                continue;
            }
            let parts: Vec<_> = name.split('-').collect();
            let valid = parts.len() == 3
                && parts
                    .iter()
                    .all(|part| !part.is_empty() && part.bytes().all(|c| c.is_ascii_digit()));
            if !valid {
                return Err(invalid("Invalid backup name."));
            }
            storage::directory(&entry.path(), true)?;
            backups.push(Backup {
                completed: history.iter().any(|r| r.backup == name),
                pending: pending.as_ref().is_some_and(|r| r.backup == name),
                name,
            });
        }
        backups.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(backups)
    }

    pub fn apply(&self, step: &dyn Step, barriers: &impl atomic_fs::Barriers) -> Result<Receipt> {
        if self.pending()?.is_some() {
            return Err(invalid("Recover the pending migration first."));
        }
        admit_writes(self.admin, self.owner, barriers)?;
        if step.from() == 0 || step.to() <= step.from() {
            return Err(invalid("Invalid forward migration."));
        }
        let paths = self.source_paths(step)?;
        let original = paths
            .iter()
            .map(|p| snapshot(&storage::scoped(self.root, p)?))
            .collect::<Result<Vec<_>>>()?;
        let backups = self.admin.join("backups");
        let migration_directory = self.admin.join("migrations");
        files::directory(&backups)?;
        files::directory(&migration_directory)?;
        let name = storage::name(step.from(), step.to())?;
        let backup = backups.join(&name);
        atomic_fs::Publisher::new(crate::names::PUBLICATION_OPTIONS)
            .with_barriers(barriers)
            .staged_dir_retaining(&backup, |stage| {
                for (path, expected) in paths.iter().zip(&original) {
                    copy(
                        &storage::scoped(self.root, path)?,
                        &storage::scoped(stage, path)?,
                        expected.as_ref(),
                    )?;
                }
                self.database_backups(step, stage)?;
                Ok(())
            })
            .map_err(std::io::Error::other)?;
        // Original bytes are durable and immutable before any transformation.
        let staged = migration_directory.join(&name);
        atomic_fs::Publisher::new(crate::names::PUBLICATION_OPTIONS)
            .with_barriers(barriers)
            .staged_dir_retaining(&staged, |stage| {
                for (path, expected) in paths.iter().zip(&original) {
                    copy(
                        &storage::scoped(&backup, path)?,
                        &storage::scoped(stage, path)?,
                        expected.as_ref(),
                    )?;
                }
                Self::stage_databases(step, &backup, stage)?;
                step.transform(stage)?;
                step.validate_after(stage)?;
                self.check_marker(stage, step.to())
            })
            .map_err(std::io::Error::other)?;
        let receipt = self.prepared_receipt(step, name, paths, original)?;
        self.check_receipt(&receipt)?;
        for change in &receipt.files {
            if snapshot(&storage::scoped(self.root, &change.path)?)? != change.before {
                return Err(invalid(
                    "External edit during migration preparation; backup retained.",
                ));
            }
        }
        (self.owner)()?;
        storage::create_record(&self.admin.join(PENDING), &receipt, barriers)?;
        self.recover(step, barriers)
    }

    fn source_paths(&self, step: &dyn Step) -> Result<Vec<PathBuf>> {
        (self.owner)()?;
        step.validate_before(self.root)?;
        let mut paths = step.files(self.root)?;
        paths.sort();
        if paths.is_empty()
            || paths.windows(2).any(|p| p[0] == p[1])
            || !paths.iter().any(|p| p == self.marker)
        {
            return Err(invalid(
                "Migration requires unique file paths and its marker.",
            ));
        }
        for path in &paths {
            storage::relative(path)?;
            self.check_file_path(path)?;
        }
        self.database_paths(step, &paths)?;
        self.check_marker(self.root, step.from())?;
        Ok(paths)
    }

    fn prepared_receipt(
        &self,
        step: &dyn Step,
        name: String,
        paths: Vec<PathBuf>,
        original: Vec<Option<Snapshot>>,
    ) -> Result<Receipt> {
        let identity = storage::identity(self.root)?;
        let staged = self.admin.join("migrations").join(&name);
        let databases = Self::database_receipts(step, &self.admin.join("backups").join(&name))?;
        Ok(Receipt {
            from: step.from(),
            to: step.to(),
            backup: name.clone(),
            stage: name,
            root_device: identity.0,
            root_inode: identity.1,
            databases,
            files: paths
                .into_iter()
                .zip(original)
                .map(|(path, before)| {
                    Ok(FileChange {
                        after: snapshot(&storage::scoped(&staged, &path)?)?,
                        path,
                        before,
                    })
                })
                .collect::<Result<_>>()?,
        })
    }

    pub fn recover(&self, step: &dyn Step, barriers: &impl atomic_fs::Barriers) -> Result<Receipt> {
        let Some(receipt) = self.pending()? else {
            return self.recover_completion(step, barriers);
        };
        if (receipt.from, receipt.to) != (step.from(), step.to()) {
            return Err(invalid(
                "The pending migration needs its registered released-format reader.",
            ));
        }
        self.validate_recovery(step, &receipt)?;
        storage::sync(&self.admin.join(PENDING), barriers)?;
        let staged = self.admin.join("migrations").join(&receipt.stage);
        // Marker publication is last, including during recovery.
        for change in receipt
            .files
            .iter()
            .filter(|c| c.path != self.marker)
            .chain(receipt.files.iter().filter(|c| c.path == self.marker))
        {
            (self.owner)()?;
            self.check_receipt(&receipt)?;
            storage::publish(self.root, &staged, change, barriers, self.owner)?;
        }
        step.validate_after(self.root)?;
        self.finish(&receipt, barriers)?;
        Ok(receipt)
    }

    fn recover_completion(
        &self,
        step: &dyn Step,
        barriers: &impl atomic_fs::Barriers,
    ) -> Result<Receipt> {
        let matching: Vec<_> = self
            .history()?
            .into_iter()
            .filter(|receipt| (receipt.from, receipt.to) == (step.from(), step.to()))
            .collect();
        let [receipt] = matching.as_slice() else {
            return Err(invalid(
                "No unique completed migration matches the requested step.",
            ));
        };
        // A removed intent can be retried only against its exact completed state.
        // Never reapply history over subsequent ordinary writes or later steps.
        self.check_marker(self.root, receipt.to)?;
        self.validate_recovery(step, receipt)?;
        step.validate_after(self.root)?;
        admit_writes(self.admin, self.owner, barriers)?;
        Ok(receipt.clone())
    }

    fn validate_recovery(&self, step: &dyn Step, receipt: &Receipt) -> Result<()> {
        let backup = self.admin.join("backups").join(&receipt.backup);
        let staged = self.admin.join("migrations").join(&receipt.stage);
        storage::directory(&backup, true)?;
        storage::directory(&staged, true)?;
        // Recheck every input before the first resumed write. A conflict in a
        // later page cannot silently permit earlier pages to advance on retry.
        for change in &receipt.files {
            if snapshot(&storage::scoped(&backup, &change.path)?)? != change.before
                || snapshot(&storage::scoped(&staged, &change.path)?)? != change.after
            {
                return Err(invalid("Migration backup or prepared content changed."));
            }
            let current = snapshot(&storage::scoped(self.root, &change.path)?)?;
            if current != change.before && current != change.after {
                return Err(invalid(
                    "External edit fences migration recovery; preserve the intent and edited \
                        file.",
                ));
            }
        }
        step.validate_before(&backup)?;
        self.check_databases(step, receipt, &backup)?;
        step.validate_after(&staged)?;
        self.check_marker(&backup, receipt.from)?;
        self.check_marker(&staged, receipt.to)?;
        let marker: hive_input::compatibility::Marker =
            storage::record(&self.root.join(self.marker))?;
        if marker.format != receipt.from && marker.format != receipt.to {
            return Err(invalid("Migration marker is inconsistent with its intent."));
        }
        if marker.format == receipt.to {
            for change in &receipt.files {
                if snapshot(&storage::scoped(self.root, &change.path)?)? != change.after {
                    return Err(invalid(
                        "Completed migration marker has incomplete content.",
                    ));
                }
            }
        }
        let expected: BTreeSet<_> = step.files(&backup)?.into_iter().collect();
        if expected != receipt.files.iter().map(|c| c.path.clone()).collect() {
            return Err(invalid(
                "Pending migration file set differs from its registered step.",
            ));
        }
        Ok(())
    }

    fn finish(&self, receipt: &Receipt, barriers: &impl atomic_fs::Barriers) -> Result<()> {
        (self.owner)()?;
        let completed = self
            .admin
            .join("migrations")
            .join(format!("{}.json", receipt.stage));
        if files::optional_metadata(&completed)?.is_some() {
            if storage::record::<Receipt>(&completed)? != *receipt {
                return Err(invalid("Migration receipt conflicts."));
            }
            storage::sync(&completed, barriers)?;
        } else {
            storage::create_record(&completed, receipt, barriers)?;
        }
        let pending = self.admin.join(PENDING);
        if storage::record::<Receipt>(&pending)? != *receipt {
            return Err(invalid("Migration intent changed."));
        }
        barriers.observe(atomic_fs::Step::BeforeRemove)?;
        (self.owner)()?;
        if storage::record::<Receipt>(&pending)? != *receipt {
            return Err(invalid("Migration intent changed."));
        }
        std::fs::remove_file(&pending)?;
        barriers.observe(atomic_fs::Step::AfterRemove)?;
        storage::sync_directory(self.admin, barriers)?;
        Ok(())
    }

    fn check_receipt(&self, receipt: &Receipt) -> Result<()> {
        (self.owner)()?;
        if storage::identity(self.root)? != (receipt.root_device, receipt.root_inode)
            || receipt.from == 0
            || receipt.to <= receipt.from
            || receipt.files.is_empty()
            || receipt.backup != receipt.stage
            || !storage::valid_name(&receipt.backup, receipt.from, receipt.to)
        {
            return Err(invalid("Invalid migration identity."));
        }
        let mut paths = BTreeSet::new();
        for change in &receipt.files {
            storage::relative(&change.path)?;
            self.check_file_path(&change.path)?;
            if !paths.insert(&change.path) {
                return Err(invalid("Duplicate migration path."));
            }
            for state in change.before.iter().chain(change.after.iter()) {
                state.validate()?;
            }
        }
        let mut databases = BTreeSet::new();
        for database in &receipt.databases {
            storage::relative(&database.path)?;
            self.check_file_path(&database.path)?;
            database.snapshot.validate()?;
            if !paths.contains(&database.path) || !databases.insert(&database.path) {
                return Err(invalid("Invalid migration database backup list."));
            }
        }
        if !paths.contains(&self.marker.to_path_buf()) {
            return Err(invalid("Migration marker is missing."));
        }
        Ok(())
    }
    fn check_file_path(&self, path: &Path) -> Result<()> {
        let target = self.root.join(path);
        if target == self.admin.join(PENDING)
            || target.starts_with(self.admin.join("backups"))
            || target.starts_with(self.admin.join("migrations"))
            || path.starts_with(preparation::DATABASES)
        {
            return Err(invalid("A format step cannot rewrite migration evidence."));
        }
        Ok(())
    }
    fn check_marker(&self, root: &Path, expected: u32) -> Result<()> {
        let marker: hive_input::compatibility::Marker =
            storage::record(&storage::scoped(root, self.marker)?)?;
        if marker.format != expected {
            return Err(invalid(
                "Migration marker disagrees with its registered step.",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
