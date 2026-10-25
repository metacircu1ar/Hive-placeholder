//! Activation of an already loaded engine's space.
use super::{Embedder, Lease};
use crate::{Comb, Result};

impl Comb {
    /// Activate an already loaded engine. HTTP is never called here.
    pub fn activate_engine(
        &mut self,
        engine: &mut (impl Embedder + ?Sized),
        replace: bool,
    ) -> Result<Lease> {
        self.ready()?;
        let space = engine.space().clone();
        space.validate()?;
        self.activate_embeddings(&space, replace)
    }
}
