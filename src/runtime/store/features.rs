//! Live capability holds do not change durable agent or message outcomes.
mod queries;
use super::{Queries, api};
use crate::error::Result;

use queries::{AGENT_HOLD, RECIPIENT_HOLD};

impl Queries {
    pub(crate) fn retained_comb_backlog(&self) -> Result<Option<api::CombBacklog>> {
        if cfg!(feature = "comb") {
            return Ok(None);
        }
        // Core has no collector writers. This immutable startup snapshot is
        // published with daemon status without adding work to each writer turn.
        self.query_row_cached(queries::RETAINED_COMB_BACKLOG_SELECT, [], |row| {
            Ok(Some(api::CombBacklog {
                collectors: super::rows::unsigned(row, 0)?,
                rows: super::rows::unsigned(row, 1)?,
                delivery_supported: false,
            }))
        })
    }
    pub(crate) fn recipient_hold(&self, id: &str) -> Result<Option<api::RequiredBuild>> {
        if cfg!(feature = "comb") {
            return Ok(None);
        }
        let held: bool = self.query_row_cached(RECIPIENT_HOLD, [id], |r| r.get(0))?;
        Ok(held.then_some(api::RequiredBuild::Comb))
    }
    pub(crate) fn agent_hold(&self, id: &str) -> Result<Option<api::RequiredBuild>> {
        if cfg!(feature = "comb") {
            return Ok(None);
        }
        let held: bool = self.query_row_cached(AGENT_HOLD, [id], |r| r.get(0))?;
        Ok(held.then_some(api::RequiredBuild::Comb))
    }
    #[cfg(not(feature = "comb"))]
    pub(crate) fn decorate_holds(&self, value: &mut serde_json::Value) -> Result<()> {
        // Only daemon-owned acceptance shapes are traversed, never output/body data.
        if let Some(id) = value.get("message_id").and_then(serde_json::Value::as_str) {
            let recipient: Option<String> =
                self.query_opt_cached(queries::DECORATE_HOLDS_SELECT, [id], |r| r.get(0))?;
            if let Some(recipient) = recipient
                && self.recipient_hold(&recipient)?.is_some()
            {
                value["held_for_build"] = api::value(api::RequiredBuild::Comb);
            }
        }
        if let Some(results) = value
            .get_mut("results")
            .and_then(serde_json::Value::as_array_mut)
        {
            for result in results {
                self.decorate_holds(result)?;
            }
        }
        Ok(())
    }
}
