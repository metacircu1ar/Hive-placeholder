//! Derived work selection and publication; no inference runs in these methods.
use super::{Applied, BATCH_SIZE, Completed, Lease, Space, normalize};
use crate::values::ActivationId;
use crate::{Comb, Error, Result, read::Publishing};

impl Comb {
    pub(crate) fn deactivate_embeddings(&mut self) -> Result<()> {
        self.ready()?;
        let publication = self.publication.clone();
        let _publishing = Publishing::derived(&publication)?;
        self.index.deactivate_embeddings()
    }
    pub fn activate_embeddings(&mut self, space: &Space, replace: bool) -> Result<Lease> {
        self.ready()?;
        space.validate()?;
        let fingerprint = space.fingerprint()?;
        if let Some((lease, _)) = self.index.active_space()? {
            if lease.fingerprint == fingerprint {
                return Ok(lease);
            }
            if !replace {
                return Err(Error::embedding_replace_required());
            }
        }
        let lease = Lease {
            fingerprint,
            activation: ActivationId::generate(),
            dimensions: space.dimensions,
        };
        let publication = self.publication.clone();
        let _publishing = Publishing::derived(&publication)?;
        self.index.activate_embeddings(&lease, space)?;
        tracing::info!(name: "comb.embedding_space_activated", count = lease.dimensions);
        Ok(lease)
    }
    pub fn apply_embeddings(&mut self, completed: Completed) -> Result<Applied> {
        self.ready()?;
        let Completed { batch, vectors } = completed;
        if batch.jobs.len() != vectors.len()
            || batch.jobs.is_empty()
            || batch.jobs.len() > BATCH_SIZE
        {
            return Err(Error::invalid_completed_batch());
        }
        // Stale work is discarded as a whole, including permanent failure reports.
        if let Err(error) = self.index.check_lease(&batch.lease) {
            if !error.is_space_change() {
                return Err(error);
            }
            return Ok(Applied {
                stale: batch.jobs.len(),
                ..Applied::default()
            });
        }
        let normalized = vectors
            .into_iter()
            .map(|v| match v {
                Ok(vector) => Ok(Ok(normalize(vector, batch.lease.dimensions)?)),
                Err(error) if error.is_input_rejection() => {
                    Ok(Err(Error::individual_input_rejected(error)))
                }
                Err(error) => Err(error),
            })
            .collect::<Result<Vec<_>>>()?;
        let publication = self.publication.clone();
        let _publishing = Publishing::derived(&publication)?;
        let applied = self.index.apply_embeddings(batch, normalized)?;
        if applied.rejected > 0 {
            tracing::warn!(name: "comb.embedding_inputs_rejected",
                 count = applied.rejected,
                 decision = "retain_failure_for_doctor");
        }
        Ok(applied)
    }
    pub fn retry_embedding_failures(&mut self) -> Result<usize> {
        self.ready()?;
        self.index.retry_embedding_failures()
    }
}
