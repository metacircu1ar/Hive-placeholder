//! Snapshot selected text before event offload, with requester-owned overflow artifacts.
use super::{Store, decode_output, queries};
use crate::{
    error::{Error, Result},
    runtime::{
        api::joins::{JoinOutput, MEMBER_BYTES},
        domain::Agent,
    },
};
use rusqlite::params;

impl Store {
    // Caller has already validated the exact message/execution/turn via call_results.
    pub(in crate::runtime::store) fn join_output(
        &self,
        message: &str,
        text: &str,
        final_answer: bool,
    ) -> Result<()> {
        if !self.joins_possible.get() {
            return Ok(());
        }
        let owner: Option<(String, String, Option<String>)> =
            self.query_opt_cached(queries::OUTPUT_OWNER, [message], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
        if let Some((join, requester, previous)) = owner {
            let output = self.join_preview(&join, &requester, text, MEMBER_BYTES)?;
            self.execute_cached(
                queries::SET_OUTPUT,
                params![
                    crate::runtime::api::value(output).to_string(),
                    final_answer,
                    message
                ],
            )?;
            self.retire_join_output(&join, &requester, previous)?;
        }
        Ok(())
    }
    /// Only the live completed-turn path supplies a message; idle completion never does.
    pub(crate) fn joined_completion(
        &self,
        agent: &Agent,
        message: Option<&str>,
        result: &str,
    ) -> Result<bool> {
        let Some(message) = message.filter(|_| self.joins_possible.get()) else {
            return Ok(false);
        };
        let owner: Option<(String, String, Option<String>, Option<String>)> = self
            .query_opt_cached(queries::COMPLETION_OWNER, params![message, agent.id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?;
        let Some((join, requester, output, previous)) = owner else {
            return Ok(false);
        };
        let used = decode_output(output)?.map_or(0, |output| output.text.len());
        let completion =
            self.join_preview(&join, &requester, result, MEMBER_BYTES.saturating_sub(used))?;
        self.execute_cached(
            queries::SET_COMPLETION,
            params![crate::runtime::api::value(completion).to_string(), message],
        )?;
        self.retire_join_output(&join, &requester, previous)?;
        Ok(agent.parent_id.as_deref() == Some(requester.as_str()))
    }
    fn retire_join_output(
        &self,
        join: &str,
        requester: &str,
        previous: Option<String>,
    ) -> Result<()> {
        let Some(target) = decode_output(previous)?.and_then(|output| output.read_target) else {
            return Ok(());
        };
        let id = target
            .strip_prefix("artifact:")
            .ok_or_else(|| Error::database_error("Invalid retained join artifact reference."))?;
        // Only remove this join's index entry. Compaction reclaims the orphan file;
        // an enclosing rollback restores the old reference and its index together.
        if self.execute_cached(queries::RETIRE_OUTPUT, params![id, join, requester])? != 1 {
            return Err(Error::database_error(
                "Invalid retained join artifact ownership.",
            ));
        }
        Ok(())
    }
    fn join_preview(
        &self,
        join: &str,
        requester: &str,
        text: &str,
        budget: usize,
    ) -> Result<JoinOutput> {
        let mut end = text.len().min(budget);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let read_target = if end == text.len() {
            None
        } else {
            let value =
                crate::runtime::api::value(crate::runtime::api::events::TextArtifact { text });
            let (id, _) = self.publish_artifact(
                &value,
                value.to_string().len(),
                (requester != "human").then_some(requester),
                Some(join),
            )?;
            Some(format!("artifact:{id}"))
        };
        Ok(JoinOutput {
            text: text[..end].to_owned(),
            read_target,
        })
    }
}
