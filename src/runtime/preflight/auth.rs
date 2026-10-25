//! Filesystem and catalog evidence with leases retained through the consuming commit.
use super::fs_context::{FsContext, MemberPurpose};
use crate::{
    error::Result,
    runtime::{
        Core,
        preflight::request::MemberFacts,
        provider::catalogs::{BindingInput, Catalogs},
    },
};
use std::sync::Arc;

pub(in crate::runtime) struct Input {
    fs: FsContext,
    target: String,
    previous: Option<crate::runtime::domain::Agent>,
}
pub(in crate::runtime) struct Prepared {
    pub members: Vec<MemberFacts>,
    discovery: Vec<(usize, BindingInput)>,
}
impl Input {
    pub fn capture(core: &Core, target: String) -> Self {
        Self {
            fs: core.fs_context(),
            target,
            previous: None,
        }
    }
    pub fn after_cleanup(mut self, agent: crate::runtime::domain::Agent) -> Self {
        self.previous = Some(agent);
        self
    }
    pub fn resolve(self) -> Result<Prepared> {
        if let Some(agent) = &self.previous
            && !agent.conversation_clean
            && agent.native_input_attempted
        {
            let binding =
                self.fs
                    .state
                    .generation(&agent.auth, &agent.profile, agent.auth_generation)?;
            self.fs
                .state
                .check_conversation(&binding, agent.provider_id())?;
        }
        let mut result = Prepared {
            members: vec![],
            discovery: vec![],
        };
        // Member resolution acquires leases and records failures in configured order.
        for auth in self.fs.config.auth_members(&self.target)? {
            let (member, discovery) = self.fs.member(auth, MemberPurpose::LoginSwitch);
            if let Some(input) = discovery {
                result.discovery.push((result.members.len(), input));
            }
            result.members.push(member);
        }
        Ok(result)
    }
}
impl Prepared {
    pub async fn discover(&mut self, catalogs: &Arc<Catalogs>) {
        for (index, input) in std::mem::take(&mut self.discovery) {
            self.members[index].catalog = Some(
                catalogs
                    .lookup_binding(input)
                    .await
                    .map(|(models, _)| models),
            );
        }
    }
    pub fn tickets(&self) -> Vec<Arc<crate::state::RuntimeLease>> {
        self.members
            .iter()
            .filter_map(|m| m.lease.clone())
            .collect()
    }
}
