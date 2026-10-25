//! Hold strict index transactions and centralize generation and namespace lookups.
use super::{Binding, Generation, Lock, State, corrupt, profiles::Index, write_json};
use crate::{config::Provider, error::Result};
use std::ops::{Deref, DerefMut};
use uuid::Uuid;

impl Binding {
    pub(super) fn same_identity(&self, other: &Self) -> bool {
        self.id == other.id && self.profile == other.profile && self.directory == other.directory
    }
}
impl Index {
    pub(super) fn generation(&self, id: Uuid) -> Result<&Generation> {
        self.generations.get(&id).ok_or_else(corrupt)
    }
    pub(super) fn into_generation(mut self, id: Uuid) -> Result<Generation> {
        self.generations.remove(&id).ok_or_else(corrupt)
    }
    pub(super) fn generation_mut(&mut self, id: Uuid) -> Result<&mut Generation> {
        self.generations.get_mut(&id).ok_or_else(corrupt)
    }
    pub(super) fn claude_namespaces(
        &self,
        excluding: Option<Uuid>,
    ) -> impl Iterator<Item = Result<String>> + '_ {
        self.generations
            .values()
            .filter(move |generation| {
                Some(generation.binding.id) != excluding
                    && generation.binding.profile.provider == Provider::Claude
            })
            .map(|generation| {
                super::generations::claude_namespace(&generation.binding.directory.join("provider"))
            })
    }
}

pub(super) struct IndexTransaction<'a> {
    state: &'a State,
    index: Index,
}
impl IndexTransaction<'_> {
    /// Commit explicitly: registration intents and credential fences may need a
    /// durable boundary before more filesystem work or before returning an error.
    pub fn save(&self) -> Result<()> {
        write_json(&self.state.root.join("index.json"), &self.index)
    }
}
impl Deref for IndexTransaction<'_> {
    type Target = Index;
    fn deref(&self) -> &Self::Target {
        &self.index
    }
}
impl DerefMut for IndexTransaction<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.index
    }
}
impl State {
    pub(super) fn update_index<T>(
        &self,
        change: impl FnOnce(&mut IndexTransaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let lock = self.metadata_lock()?;
        self.update_locked_index(&lock, change)
    }
    /// Registration acquires its index lock after creating the private root.
    pub(super) fn update_locked_index<T>(
        &self,
        _lock: &Lock,
        change: impl FnOnce(&mut IndexTransaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let mut transaction = IndexTransaction {
            state: self,
            index: self.read_index()?,
        };
        change(&mut transaction)
    }
}

#[cfg(test)]
mod tests;
