//! Canonical process identity depends only on effective catalog values.
use super::{Scope, Snapshot, catalog};
use crate::{config::Provider, error::Result};
use sha2::{Digest, Sha256};

impl Snapshot {
    /// Empty settings keep the existing default sharing identity.
    pub(crate) fn process_key(&self, provider: Provider) -> Result<String> {
        let catalog = catalog(provider)?;
        let mut digest = None;
        for (key, value) in &self.values {
            if catalog.entry(key)?.scope == Scope::Process {
                let digest = digest.get_or_insert_with(Sha256::new);
                digest.update(key.as_bytes());
                digest.update([0]);
                digest.update(value.to_string().as_bytes());
                digest.update([0]);
            }
        }
        Ok(digest.map_or_else(String::new, |digest| format!("{:x}", digest.finalize())))
    }
}
