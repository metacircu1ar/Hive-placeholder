//! Indexed, transaction-owned logical process-variant admission.
mod queries;
use super::{Queries, numeric};
use crate::{error::Result, runtime::domain::Agent};
use rusqlite::params;
use std::collections::BTreeSet;

impl Queries {
    /// The caller holds the writer transaction and has not saved this candidate.
    pub(in crate::runtime) fn admits_process_options(
        &self,
        agent: &Agent,
        limit: usize,
    ) -> Result<bool> {
        let desired = agent
            .agent_options
            .desired_ref()
            .process_key(agent.profile.provider)?;
        let reserved = agent
            .reserved_turn
            .as_ref()
            .map(|reserved| {
                reserved
                    .selection
                    .options
                    .process_key(agent.profile.provider)
            })
            .transpose()?;
        self.admits_process_keys(
            &agent.auth,
            Some(&agent.id),
            &desired,
            reserved.as_deref(),
            limit,
        )
    }

    /// Admission before spawn has an identity, or for a move to another login.
    pub(in crate::runtime) fn admits_process_keys(
        &self,
        auth: &str,
        agent: Option<&str>,
        desired: &str,
        reserved: Option<&str>,
        limit: usize,
    ) -> Result<bool> {
        let keys: Vec<String> = self.sql().cached().query_column(
            queries::ADMITS_PROCESS_KEYS_SELECT,
            params![
                auth,
                agent,
                i64::try_from(limit.saturating_add(1)).map_err(numeric)?
            ],
        )?;
        let mut keys: BTreeSet<_> = keys.into_iter().collect();
        keys.insert(desired.to_owned());
        if let Some(reserved) = reserved {
            keys.insert(reserved.to_owned());
        }
        Ok(keys.len() <= limit)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::runtime::{
        domain::{AgentState, ReservedTurn, TaskOutcome},
        store::Store,
        test_support::AgentBuilder,
    };
    fn compression(agent: &mut Agent, value: bool) {
        agent.agent_options = agent
            .agent_options
            .changed(
                agent.profile.provider,
                &[("codex.request_compression".into(), serde_json::json!(value))].into(),
                None,
            )
            .unwrap();
    }
    #[test]
    fn variant_claims_track_reservations_ending_auth_moves_and_transaction_rollback() {
        let temp = crate::runtime::test_directory();
        let store = Store::open(&temp.path().join("db")).unwrap();
        let mut a = AgentBuilder::new("a").build();
        store.save(&a).unwrap();
        a.reserved_turn = Some(ReservedTurn {
            message_id: "m".into(),
            selection: a.desired_selection(),
        });
        compression(&mut a, true);
        assert!(!store.admits_process_options(&a, 1).unwrap());
        assert!(store.admits_process_options(&a, 2).unwrap());
        store.save(&a).unwrap();
        // The two claims survive a fresh connection and disappear atomically.
        drop(store);
        let store = Store::open(&temp.path().join("db")).unwrap();
        let mut b = AgentBuilder::new("b").build();
        compression(&mut b, false);
        assert!(!store.admits_process_options(&b, 2).unwrap());
        store.begin().unwrap();
        a.reserved_turn = None;
        store.save(&a).unwrap();
        assert!(store.admits_process_options(&b, 2).unwrap());
        store.rollback().unwrap();
        assert!(!store.admits_process_options(&b, 2).unwrap());
        a.auth = "destination".into();
        store.save(&a).unwrap();
        assert!(store.admits_process_options(&b, 1).unwrap());
        b.auth = "destination".into();
        assert!(!store.admits_process_options(&b, 1).unwrap());
        a.task_outcome = TaskOutcome::Completed;
        a.state = AgentState::Stopped;
        store.save(&a).unwrap();
        assert!(store.admits_process_options(&b, 1).unwrap());
        let count: i64 = store
            .query_row_cached("SELECT count(*) FROM process_variants", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
}
