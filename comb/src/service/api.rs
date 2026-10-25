//! Server execution of shared wire operations.
pub(crate) use actions::{Capability, Class, Dispatch, ReadAction};
pub use comb_protocol::service::api::Operation;
pub(crate) use comb_protocol::service::api::{Capabilities, Reply, Request, actions};
mod execute;
mod mutation;
use super::auth;
#[cfg(test)]
use crate::Error;
use crate::Result;
#[cfg(test)]
use crate::search::{Mode, Search};
pub(super) use execute::health;
pub(crate) use execute::{AdminPreparation, ReadExecution};
pub(crate) use mutation::MutationPreparation;
pub(crate) trait OperationAdmission {
    fn collector_admission(&self, authority: &auth::Authority) -> Result<()>;
}
impl OperationAdmission for Operation {
    fn collector_admission(&self, authority: &auth::Authority) -> Result<()> {
        if matches!(
            self.dispatch(),
            Dispatch::Read(ReadAction::Capabilities | ReadAction::Health)
        ) {
            return Ok(());
        }
        authority.collector_admission(self.required())
    }
}
#[cfg(test)]
mod tests;
