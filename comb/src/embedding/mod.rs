//! Bounded embedding work. A lease fences both document and query results.
mod backend;
mod backfill;
mod config;
mod device;
mod equivalence;
mod identity;
pub(crate) mod limits;
mod offline;
mod pieces;
mod store;
#[cfg(test)]
mod tests;
mod tokenizer;

use crate::{
    Error, Result,
    values::{ActivationId, ContentHash, Fingerprint, Slug},
};
pub use backend::Engine;
pub use backfill::Backfill;
pub(crate) use backfill::{BackfillHost, BatchSizer};
pub use config::{Config, HttpConfig};
pub use device::{RuntimeInfo, RuntimeProvider};
use serde::{Deserialize, Serialize};

pub use limits::{BATCH_SIZE, CONFIG_BYTES, INPUT_BYTES};

/// Every value that changes the meaning of a vector participates in identity.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Space {
    pub(crate) backend: String,
    pub(crate) model: String,
    pub(crate) revision: String,
    pub(crate) dimensions: usize,
    pub(crate) document_prefix: String,
    pub(crate) query_prefix: String,
    pub(crate) preprocessing: String,
    pub(crate) normalization: String,
}
impl Space {
    pub fn fingerprint(&self) -> Result<Fingerprint> {
        Ok(Fingerprint::digest(crate::error::json_bytes(self)?))
    }
    pub fn dimensions(&self) -> usize {
        self.dimensions
    }
    pub(crate) fn validate(&self) -> Result<()> {
        if !(limits::DIMENSIONS_MIN..=limits::DIMENSIONS_MAX).contains(&self.dimensions)
            || [
                &self.backend,
                &self.model,
                &self.revision,
                &self.preprocessing,
                &self.normalization,
            ]
            .iter()
            .any(|s| s.is_empty() || s.len() > limits::IDENTITY_BYTES)
            || self.document_prefix.len() > limits::PREFIX_BYTES
            || self.query_prefix.len() > limits::PREFIX_BYTES
        {
            return Err(Error::invalid_embedding_space());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Lease {
    pub(crate) fingerprint: Fingerprint,
    pub(crate) activation: ActivationId,
    pub(crate) dimensions: usize,
}
#[derive(Clone, Debug)]
pub struct Job {
    pub slug: Slug,
    pub ordinal: u32,
    pub(crate) id: i64,
    pub(crate) instance: String,
    pub(crate) input_hash: ContentHash,
    pub(crate) input: String,
}
#[derive(Clone, Debug)]
pub struct Batch {
    pub(crate) lease: Lease,
    pub jobs: Vec<Job>,
}
/// One bounded pass over current chunk IDs. Exhaustion resets the cursor for a
/// subsequent pass; retry retains the position before an uncompleted batch.
#[derive(Default)]
pub struct Scan {
    pub(crate) activation: Option<ActivationId>,
    pub(crate) after: i64,
    #[cfg(test)]
    pub(crate) steps: u64,
}
impl Scan {
    /// Reselect only this batch after a yield or transient inference failure.
    pub fn retry(&mut self, batch: &Batch) {
        if self.activation.as_ref() == Some(&batch.lease.activation)
            && let Some(first) = batch.jobs.first()
        {
            self.after = first.id - 1;
        }
    }
}
pub struct Completed {
    pub(crate) batch: Batch,
    pub(crate) vectors: Vec<Result<Vec<f32>>>,
}
pub struct Query {
    pub(crate) lease: Lease,
    pub(crate) vector: Vec<f32>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Coverage {
    pub total: u64,
    pub embedded: u64,
    pub pending: u64,
    pub rejected: u64,
}
#[derive(Default, Debug, Serialize)]
pub struct Applied {
    pub embedded: usize,
    pub rejected: usize,
    pub stale: usize,
}

/// Called only outside the Comb writer/transaction. Inputs already include
/// their space's document/query prefix. Only permanent input errors are per-row;
/// transport or invalid provider replies fail the batch without poisoning jobs.
pub trait Embedder: Send {
    fn space(&self) -> &Space;
    fn runtime(&self) -> Option<RuntimeInfo> {
        None
    }
    fn embed(&mut self, inputs: &[String]) -> Result<Vec<Result<Vec<f32>>>>;
    /// HTTP can share its connection pool without serializing document/query I/O.
    fn query_engine(&self) -> Option<Box<dyn Embedder>> {
        None
    }
}
pub fn run_batch(embedder: &mut (impl Embedder + ?Sized), batch: Batch) -> Result<Completed> {
    check_engine(embedder, &batch.lease)?;
    if batch.jobs.is_empty() || batch.jobs.len() > BATCH_SIZE {
        return Err(Error::embedding_job_count());
    }
    let inputs = batch
        .jobs
        .iter()
        .map(|j| format!("{}{}", embedder.space().document_prefix, j.input))
        .collect::<Vec<_>>();
    let mut vectors = embedder.embed(&inputs)?;
    if vectors.len() != batch.jobs.len() {
        return Err(Error::embedding_batch_result_count());
    }
    // A row over the model's limit is embedded in pieces instead of rejected.
    for (job, row) in batch.jobs.iter().zip(vectors.iter_mut()) {
        *row = match std::mem::replace(row, Ok(Vec::new())) {
            Err(rejected) if rejected.is_input_rejection() => {
                pieces::embed(embedder, &job.input, rejected)?
            }
            other => other,
        };
    }
    Ok(Completed { batch, vectors })
}
pub fn run_query(
    embedder: &mut (impl Embedder + ?Sized),
    lease: Lease,
    text: &str,
) -> Result<Query> {
    run_queries(embedder, lease, &[text.to_owned()])?
        .pop()
        .ok_or_else(Error::query_result_missing)?
}
pub(crate) fn validate_query(text: &str) -> Result<()> {
    if text.trim().is_empty() || text.len() > crate::search::limits::QUERY_BYTES {
        return Err(Error::invalid_query());
    }
    Ok(())
}
pub(crate) fn run_queries(
    embedder: &mut (impl Embedder + ?Sized),
    lease: Lease,
    texts: &[String],
) -> Result<Vec<Result<Query>>> {
    check_engine(embedder, &lease)?;
    if texts.is_empty() || texts.len() > BATCH_SIZE {
        return Err(Error::query_input_count());
    }
    let inputs = texts
        .iter()
        .map(|text| {
            validate_query(text)?;
            Ok(format!("{}{}", embedder.space().query_prefix, text))
        })
        .collect::<Result<Vec<_>>>()?;
    let vectors = embedder.embed(&inputs)?;
    if vectors.len() != inputs.len() {
        return Err(Error::query_result_count());
    }
    Ok(vectors
        .into_iter()
        .map(|vector| {
            Ok(Query {
                lease: lease.clone(),
                vector: normalize(vector?, lease.dimensions)?,
            })
        })
        .collect())
}
fn check_engine(embedder: &(impl Embedder + ?Sized), lease: &Lease) -> Result<()> {
    if embedder.space().fingerprint()? != lease.fingerprint
        || embedder.space().dimensions != lease.dimensions
    {
        return Err(Error::embedder_space_mismatch());
    }
    Ok(())
}
pub(crate) fn normalize(mut vector: Vec<f32>, dimensions: usize) -> Result<Vec<f32>> {
    if vector.len() != dimensions || vector.iter().any(|v| !v.is_finite()) {
        return Err(Error::invalid_vector_components());
    }
    let norm = vector
        .iter()
        .map(|v| f64::from(*v).powi(2))
        .sum::<f64>()
        .sqrt();
    if norm == 0.0 || !norm.is_finite() {
        return Err(Error::invalid_vector_norm());
    }
    for value in &mut vector {
        *value = crate::convert::f64_f32(f64::from(*value) / norm);
    }
    Ok(vector)
}
pub(crate) fn decode(bytes: &[u8], dimensions: usize) -> Result<Vec<f32>> {
    if bytes.len() != dimensions * 4 {
        return Err(Error::cached_vector_size());
    }
    let vector = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect::<Vec<_>>();
    let norm = vector.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>();
    if !norm.is_finite() || (norm - 1.0).abs() > limits::UNIT_NORM_TOLERANCE {
        return Err(Error::cached_vector_norm());
    }
    Ok(vector)
}
