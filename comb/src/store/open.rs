use super::{Comb, Disk, Index, Path, Result};
use crate::disk::Recovery;
use crate::error::CommitExt;

impl Comb {
    pub fn upgrade(root: &Path, dry_run: bool) -> Result<crate::UpgradeReport> {
        crate::disk::upgrade(root, dry_run)
    }
    pub fn init(root: &Path) -> Result<Self> {
        Self::open_inner(root, true, false)
    }
    pub fn open(root: &Path) -> Result<Self> {
        Self::open_inner(root, false, false)
    }
    pub fn rebuild(root: &Path) -> Result<Self> {
        Self::open_inner(root, false, true)
    }
    fn open_inner(root: &Path, initialize: bool, rebuild: bool) -> Result<Self> {
        let disk = Disk::acquire(root, initialize)?;
        Self::from_disk(disk, initialize, rebuild)
    }
    pub(crate) fn open_checked(root: &Path, check: &impl Fn(&Disk) -> Result<()>) -> Result<Self> {
        let disk = Disk::acquire(root, false)?;
        check(&disk)?;
        Self::from_disk(disk, false, false)
    }
    fn from_disk(disk: Disk, initialize: bool, rebuild: bool) -> Result<Self> {
        let recovering = disk.recover()?;
        let recovery_accepted = recovering.is_some();
        if recovery_accepted {
            tracing::info!(name: "comb.recovery_started", decision = "reconcile_index");
        }
        let result = (|| {
            let accepted = disk.rebuild_index(rebuild)?;
            if accepted {
                tracing::info!(name: "comb.index_rebuild", decision = "populate_index");
            }
            let result = Self::populate(disk, initialize, recovering);
            if accepted { result.committed() } else { result }
        })();
        if recovery_accepted {
            result.committed()
        } else {
            result
        }
    }
    fn populate(disk: Disk, initialize: bool, recovering: Option<Recovery>) -> Result<Self> {
        let index_path = disk.index_path();
        let populating = disk.index_population(!crate::disk::exists(&index_path)?)?;
        let needs_sync = initialize || populating;
        if populating {
            tracing::info!(name: "comb.index_population_started", decision = "populate_index");
        }
        let preserved_space = disk.population_space()?;
        let mut index = Index::open(&index_path)?;
        if let Some(space) = &preserved_space {
            index.restore_space(space)?;
        }
        let health = std::sync::Arc::new(crate::locks::Health::default());
        let mut comb = Self {
            disk,
            index,
            dirty: false,
            embedding_worker_error: None,
            embedding_runtime: None,
            publication: std::sync::Arc::new(crate::read::Publication::new(health.clone())),
            health,
            #[cfg(test)]
            before_index: None,
        };
        // Reconcile the interrupted mutation's derived index before admitting
        // another file write. Its intent remains durable until this succeeds.
        if let Some(recovering) = recovering {
            match recovering {
                Recovery::Single(slug) => {
                    if let Some(loaded) = comb.disk.load(&slug)? {
                        comb.index.upsert(&slug, &loaded.raw, &loaded.page)?;
                    } else {
                        comb.index.delete(&slug)?;
                    }
                }
                Recovery::Consolidation(pair) => {
                    comb.index.upsert_consolidation(&pair)?;
                    pair.completed(&comb.disk)?;
                }
            }
            comb.disk.finish()?;
            tracing::info!(name: "comb.recovery_finished", success = true);
        }
        if needs_sync {
            comb.sync()?;
        }
        if populating {
            comb.disk.finish_population()?;
            tracing::info!(name: "comb.index_population_finished", success = true);
        }
        comb.disk.cleanup_preparations()?;
        tracing::info!(name: "comb.opened", success = true);
        Ok(comb)
    }
}
