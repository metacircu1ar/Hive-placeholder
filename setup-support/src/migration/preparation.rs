use super::{DatabaseBackup, Engine, Receipt, Step, database, storage};
use crate::{Result, files, invalid};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

pub(super) const DATABASES: &str = ".database-backups";
const COMPANIONS: &[&str] = &["-wal", "-journal", "-shm"];

impl Engine<'_> {
    pub(super) fn database_paths(&self, step: &dyn Step, files: &[PathBuf]) -> Result<()> {
        let mut seen = BTreeSet::new();
        for path in step.databases() {
            storage::relative(&path)?;
            self.check_file_path(&path)?;
            if !seen.insert(path.clone()) || !files.contains(&path) {
                return Err(invalid("Invalid database migration path."));
            }
            for suffix in COMPANIONS {
                if !files.contains(&companion(&path, suffix)) {
                    return Err(invalid("Database migration omits a companion file."));
                }
            }
        }
        Ok(())
    }

    pub(super) fn database_backups(&self, step: &dyn Step, originals: &Path) -> Result<()> {
        for path in step.databases() {
            let target = originals.join(DATABASES).join(&path);
            files::directory(
                target
                    .parent()
                    .ok_or_else(|| invalid("Missing database backup parent."))?,
            )?;
            database::Copy::open(&storage::scoped(originals, &path)?)?.vacuum(
                &target,
                self.owner,
                |source, target| step.vacuum(source, target),
            )?;
        }
        Ok(())
    }

    pub(super) fn stage_databases(step: &dyn Step, backup: &Path, stage: &Path) -> Result<()> {
        for path in step.databases() {
            let source = storage::scoped(&backup.join(DATABASES), &path)?;
            storage::copy(
                &source,
                &storage::scoped(stage, &path)?,
                storage::snapshot(&source)?.as_ref(),
            )?;
            for suffix in COMPANIONS {
                let companion = storage::scoped(stage, &companion(&path, suffix))?;
                if files::optional_metadata(&companion)?.is_some() {
                    std::fs::remove_file(&companion)?;
                    files::sync_parent(&companion)?;
                }
            }
        }
        Ok(())
    }

    pub(super) fn database_receipts(step: &dyn Step, backup: &Path) -> Result<Vec<DatabaseBackup>> {
        step.databases()
            .into_iter()
            .map(|path| {
                let snapshot =
                    storage::snapshot(&storage::scoped(&backup.join(DATABASES), &path)?)?
                        .ok_or_else(|| invalid("Missing SQLite backup."))?;
                Ok(DatabaseBackup { path, snapshot })
            })
            .collect()
    }

    pub(super) fn check_databases(
        &self,
        step: &dyn Step,
        receipt: &Receipt,
        backup: &Path,
    ) -> Result<()> {
        self.database_paths(
            step,
            &receipt
                .files
                .iter()
                .map(|c| c.path.clone())
                .collect::<Vec<_>>(),
        )?;
        if Self::database_receipts(step, backup)? != receipt.databases {
            return Err(invalid(
                "SQLite backup changed or differs from the registered step.",
            ));
        }
        Ok(())
    }
}

fn companion(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    name.into()
}
