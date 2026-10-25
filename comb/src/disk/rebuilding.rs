//! A durable, identity-bound deletion intent for regenerable SQLite files.
use super::{Disk, atomic_admin, exists, read};
use crate::{Error, Result, error::CommitExt, index::Index, values::CombId};
use serde::{Deserialize, Serialize};

mod files;
use files::{Directory, Identity};
#[cfg(test)]
mod tests;

const INTENT: &str = "index-rebuild.json";
const POPULATION: &str = "index-population.json";
const NAMES: [&str; 8] = [
    "index.sqlite",
    "index.sqlite-wal",
    "index.sqlite-shm",
    "index.sqlite-journal",
    "vectors.sqlite",
    "vectors.sqlite-wal",
    "vectors.sqlite-shm",
    "vectors.sqlite-journal",
];

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
    RebuildIndex,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    operation: Operation,
    comb: CombId,
    parent: Identity,
    reset_vectors: bool,
    #[serde(deserialize_with = "super::required_option")]
    space: Option<crate::embedding::Space>,
    files: [Option<Identity>; 8],
}

impl Disk {
    pub(crate) fn reject_pending_rebuild(&self) -> Result<()> {
        if self.rebuild_intent()?.is_some()
            || (self.index_population(false)? && !exists(&self.index_path())?)
        {
            return Err(Error::pending_state());
        }
        Ok(())
    }

    pub(crate) fn rebuild_index(&self, requested: bool) -> Result<bool> {
        let pending = self.rebuild_intent()?;
        let accepted = pending.is_some();
        let result = self.apply_rebuild(pending, requested);
        if accepted { result.committed() } else { result }
    }

    fn apply_rebuild(&self, pending: Option<Intent>, requested: bool) -> Result<bool> {
        let populating = self.index_population(false)?;
        if pending.is_none() && populating {
            // A prior attempt may have removed the deletion intent but failed
            // its final handoff barrier. Repeat that barrier before SQLite.
            let directory = Directory::open(&self.admin).committed()?;
            drop(
                directory
                    .sync_record(POPULATION, self.durability.as_ref())
                    .committed()?,
            );
        }
        let automatic = pending.is_none()
            && !requested
            && !populating
            && Index::needs_automatic_rebuild(&self.index_path())?;
        if pending.is_none() && !requested && !automatic {
            return Ok(populating);
        }
        // Preserve malformed-population refusal before destructive admission.
        let intent = match pending {
            Some(intent) => intent,
            None => self.begin_rebuild(automatic)?,
        };
        self.finish_rebuild(&intent).committed()?;
        Ok(true)
    }

    fn rebuild_intent(&self) -> Result<Option<Intent>> {
        let path = self.admin.join(INTENT);
        if !exists(&path)? {
            return Ok(None);
        }
        let record: Intent = serde_json::from_slice(&read(&path, super::RECORD_BYTES)?)?;
        if let Some(space) = &record.space {
            space.validate()?;
        }
        let directory = Directory::open(&self.admin)?;
        if record.comb != self.id() || record.parent != directory.identity() {
            return Err(Error::unsafe_index_removal());
        }
        directory.validate(&record)?;
        Ok(Some(record))
    }

    fn begin_rebuild(&self, automatic: bool) -> Result<Intent> {
        // SQLite may perform native journal recovery while inspecting vectors.
        // All those connections close before the following identity snapshot.
        let reset_vectors = if automatic {
            Index::require_preservable_vectors(&self.index_path())?;
            false
        } else {
            Index::rebuild_vectors(&self.index_path())?
        };
        let space = Index::rebuild_space(&self.index_path(), automatic)?;
        let directory = Directory::open(&self.admin)?;
        let record = Intent {
            operation: Operation::RebuildIndex,
            comb: self.id(),
            parent: directory.identity(),
            reset_vectors,
            space,
            files: directory.capture()?,
        };
        let bytes = crate::error::json_bytes(&record)?;
        if bytes.len() > super::RECORD_BYTES {
            return Err(Error::intent_size_limit());
        }
        atomic_admin(&self.admin.join(INTENT), &bytes, self.durability.as_ref())?;
        Ok(record)
    }

    fn finish_rebuild(&self, intent: &Intent) -> Result<()> {
        let directory = Directory::open(&self.admin)?;
        directory.validate(intent)?;
        // A visible intent may precede a failed publication barrier.
        drop(directory.sync_record(INTENT, self.durability.as_ref())?);
        for (slot, name) in NAMES.iter().enumerate() {
            if slot < 4 || intent.reset_vectors {
                directory.remove(name, intent.files[slot], self.durability.as_ref())?;
            }
        }
        directory.validate(intent)?;
        self.start_population(intent.space.clone())?;
        let population = directory.sync_record(POPULATION, self.durability.as_ref())?;
        let identity = directory
            .entry(INTENT)?
            .ok_or_else(Error::unsafe_index_removal)?;
        directory.remove(INTENT, Some(identity), self.durability.as_ref())?;
        // Flush the handoff after removing the deletion intent, before any new
        // SQLite generation can be created. An old intent must not reappear.
        directory.finish_handoff(&population, self.durability.as_ref())
    }
}
