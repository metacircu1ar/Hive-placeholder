//! Caller identity and execution ownership checks.
use super::Core;
use crate::error::{Error, Result};

#[derive(Clone, Debug)]
pub(crate) enum Caller {
    Human,
    System,
    Agent {
        id: String,
        execution: String,
        turn: Option<String>,
    },
}
impl Caller {
    pub(in crate::runtime) fn name(&self) -> &str {
        match self {
            Self::Human => "human",
            Self::System => "system",
            Self::Agent { id, .. } => id,
        }
    }
    pub(in crate::runtime) fn is_human(&self) -> bool {
        matches!(self, Self::Human)
    }
    pub(in crate::runtime) fn is_system(&self) -> bool {
        matches!(self, Self::System)
    }
    pub(in crate::runtime) fn agent_id(&self) -> Option<&str> {
        match self {
            Self::Agent { id, .. } => Some(id),
            _ => None,
        }
    }
    #[cfg(test)]
    pub(in crate::runtime) fn test_agent(id: &str) -> Self {
        Self::Agent {
            id: id.into(),
            execution: String::new(),
            turn: None,
        }
    }
    pub(in crate::runtime) fn check(&self, core: &Core) -> Result<()> {
        match self {
            Self::Human | Self::System => {
                if core.shutdown().load(std::sync::atomic::Ordering::Relaxed) {
                    return Err(Error::daemon_stopping("Daemon is shutting down."));
                }
            }
            Self::Agent { .. } => self.check_read(core.store())?,
        }
        Ok(())
    }
    pub(in crate::runtime) fn check_read(
        &self,
        queries: &crate::runtime::store::Queries,
    ) -> Result<()> {
        self.checked_agent(queries).map(|_| ())
    }
    pub(in crate::runtime) fn checked_agent(
        &self,
        queries: &crate::runtime::store::Queries,
    ) -> Result<super::domain::Agent> {
        let Self::Agent {
            id,
            execution,
            turn,
        } = self
        else {
            return Err(Error::unsupported_operation(
                "Agent reader ownership is required.",
            ));
        };
        let agent = queries.agent(id)?;
        if agent.execution_id.as_deref() != Some(execution) {
            return Err(Error::stale_execution(
                "Execution no longer owns this agent.",
            ));
        }
        if agent.turn_id != *turn {
            return Err(Error::stale_execution(
                "Tool call belongs to an inactive turn.",
            ));
        }
        Ok(agent)
    }
}
impl serde::Serialize for Caller {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}
