//! Read agent records and persist agent state, recovery and admission changes.
use super::{
    Agent, CHILD_PAGE_ROWS, Queries, Store, api, numeric, numeric_range, queries,
    queries as shared_queries, rows, schema, sql::FromRow as _,
};
use crate::{
    convert,
    error::{Error, Result},
    state,
};
use rusqlite::params;
use serde_json::Value;
// Provider placement's coordinator/mail SQL yields priorities 0, 1 and 2.
const MAX_PLACEMENT_PRIORITY: usize = 2;
// The fields `agent_records` sets on `data`, in `AGENT_ROW` column order.
pub(super) const RECORD_FIELDS: [&str; 25] = [
    "branch_hold",
    "branch_phase",
    "ended_at",
    "parent_id",
    "auth",
    "auth_group",
    "auth_checked_revision",
    "waiting_reason",
    "quota_wait_until",
    "lifetime",
    "state",
    "task_outcome",
    "paused",
    "expired",
    "thread_id",
    "execution_id",
    "turn_id",
    "control",
    "completion_pending",
    "result",
    "error",
    "provider_id",
    "provider_pid",
    "provider_spawn_before",
    "provider_spawn_after",
];
// The view renders these as booleans: true only for 1.
const FLAG_FIELDS: [&str; 2] = ["paused", "expired"];

/// An `AGENT_ROW` result: the stored `data` and the record columns.
pub(super) fn record_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(String, Vec<rusqlite::types::Value>)> {
    let columns = (1..=RECORD_FIELDS.len())
        .map(|index| row.get(index))
        .collect::<rusqlite::Result<_>>()?;
    Ok((row.get(0)?, columns))
}

/// One agent's `agent_records` text without the view's `json_set`, which took a
/// quarter of the writer's time in a 10k pair run. `save_at` strips every
/// record field from `data`, so appending them gives the view's object; a
/// stray copy fails the strict decoder as a duplicate field.
pub(super) fn record_text(data: &str, columns: Vec<rusqlite::types::Value>) -> Result<String> {
    use rusqlite::types::Value as Column;
    let invalid = || Error::database_error("Invalid saved agent record.");
    let body = data
        .strip_suffix('}')
        .filter(|body| body.starts_with('{'))
        .ok_or_else(invalid)?;
    if columns.len() != RECORD_FIELDS.len() {
        return Err(invalid());
    }
    let mut text = String::with_capacity(data.len() + 512);
    text.push_str(body);
    let mut separator = body.trim_end() != "{";
    for (name, column) in RECORD_FIELDS.iter().zip(columns) {
        let value = match column {
            column if FLAG_FIELDS.contains(name) => Value::Bool(column == Column::Integer(1)),
            Column::Null => Value::Null,
            Column::Integer(number) => Value::from(number),
            Column::Text(text) => Value::String(text),
            Column::Real(_) | Column::Blob(_) => return Err(invalid()),
        };
        if separator {
            text.push(',');
        }
        separator = true;
        text.push('"');
        text.push_str(name);
        text.push_str("\":");
        text.push_str(&value.to_string());
    }
    text.push('}');
    Ok(text)
}
pub(super) fn decode_agent(raw: &str) -> Result<Agent> {
    let agent: Agent = crate::runtime::domain::decode::from_str(raw)
        .map_err(|cause| Error::database_error("Invalid saved agent record.").with_source(cause))?;
    let checked = (|| {
        agent
            .agent_options
            .validate_stored(agent.profile.provider)?;
        let desired = agent.agent_options.desired_ref();
        if desired.string("model") != Some(agent.model.as_str())
            || desired.string("effort") != Some(agent.effort.as_str())
        {
            return Err(Error::database_error(
                "Stored agent options disagree with its model or effort.",
            ));
        }
        for selection in [
            agent.reserved_turn.as_ref().map(|r| &r.selection),
            agent.session_selection.as_ref(),
            agent.last_accepted_selection.as_ref(),
            agent.turn_selection.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            // The desired snapshot was validated above; equality shares that proof.
            if selection.options.as_ref() != desired.as_ref() {
                selection.options.validate_stored(agent.profile.provider)?;
            }
            if selection.options.string("model") != Some(selection.model.as_str())
                || selection.options.string("effort") != Some(selection.effort.as_str())
            {
                return Err(Error::database_error(
                    "Stored option selection disagrees with its model or effort.",
                ));
            }
        }
        Ok(())
    })();
    checked.map_err(|cause| {
        Error::database_error("Invalid stored agent options.").with_source(cause)
    })?;
    Ok(agent)
}
impl Queries {
    pub(in crate::runtime) fn relative_target(
        &self,
        caller: &crate::runtime::caller::Caller,
        target: &str,
        op: api::InputContext,
    ) -> Result<Agent> {
        if !matches!(target, "@parent" | "@child") {
            return self.agent(target);
        }
        let caller = caller.agent_id().ok_or_else(|| {
            op.error(
                hive_input::Code::InvalidValue,
                "/target",
                "an agent ID; @parent and @child require a hosted agent caller",
            )
        })?;
        let id = if target == "@parent" {
            self.agent(caller)?.parent_id.ok_or_else(|| {
                op.error(
                    hive_input::Code::InvalidValue,
                    "/target",
                    "an agent ID; this caller has no delegation parent",
                )
            })?
        } else {
            let children: Vec<String> = self
                .sql()
                .cached()
                .query_column(queries::RELATIVE_CHILDREN, [caller])?;
            match children.as_slice() {
                [id] => id.clone(),
                _ => {
                    return Err(op.error(
                        hive_input::Code::InvalidValue,
                        "/target",
                        "an agent ID or exactly one retained direct child; use hive_agents \
                         with parent set to your ID to list children",
                    ));
                }
            }
        };
        self.agent(&id)
    }

    pub(in crate::runtime) fn parent_filter_exists(&self, id: &str) -> Result<bool> {
        self.query_row_cached(queries::PARENT_FILTER_EXISTS, [id], |r| r.get(0))
    }

    pub(in crate::runtime) fn auth_filter_exists(&self, name: &str) -> Result<bool> {
        self.query_row_cached(queries::AUTH_FILTER_EXISTS, [name], |r| r.get(0))
    }

    pub(super) fn claimed(&self, id: &str) -> Result<bool> {
        self.query_row_cached(queries::AGENT_CLAIMED, [id], |r| r.get(0))
    }

    pub fn active_count(&self) -> Result<usize> {
        self.query_row_cached(queries::ACTIVE_AGENT_COUNT, [], |r| rows::count(r, 0))
    }
    pub fn controls(&self) -> Result<Vec<String>> {
        self.sql()
            .cached()
            .query_column(queries::PENDING_CONTROLS, [])
    }
    // Provider placement stays independent of the four admission classes.
    // Its coordinator/mail expression still returns values 0..2.
    pub fn placement(&self, id: &str) -> Result<usize> {
        let priority =
            self.query_row_cached(queries::AGENT_PRIORITY, [id], |r| rows::count(r, 0))?;
        if priority > MAX_PLACEMENT_PRIORITY {
            return Err(numeric_range());
        }
        Ok(priority)
    }
    pub fn provider_agent_count(&self, provider: &str) -> Result<usize> {
        self.sql()
            .map_error(|cause| {
                Error::database_error("Cannot restore provider reservations.").with_source(cause)
            })
            .query_row(queries::PROVIDER_AGENT_COUNT, [provider], |row| {
                rows::count(row, 0)
            })
    }
    pub fn children(&self, id: &str) -> Result<Vec<Agent>> {
        let limit = convert::capped_i64(CHILD_PAGE_ROWS as u64);
        self.sql()
            .cached()
            .with_rows(queries::DEPENDENT_CHILDREN, params![id, limit], |rows| {
                let mut children = Vec::new();
                while let Some(row) = rows.next::<rows::AgentRecord>()? {
                    let child = decode_agent(&row.data)?;
                    if child.id != row.id
                        || child.parent_id.as_deref() != Some(id)
                        || child.lifetime != crate::config::Lifetime::WithParent
                        || child.expired
                    {
                        return Err(Error::database_error("Invalid dependent child record."));
                    }
                    children.push(child);
                }
                Ok(children)
            })
    }
    pub fn instructions(&self, id: &str) -> Result<String> {
        self.query_opt_cached(shared_queries::AGENT_INSTRUCTIONS, [id], |r| r.get(0))
            .map(std::option::Option::unwrap_or_default)
    }
    pub fn agent(&self, id: &str) -> Result<Agent> {
        let row = self.query_opt_cached(shared_queries::AGENT_ROW, [id], record_row)?;
        let (data, columns) = match row {
            Some(row) => row,
            None if self.was_purged(id)? => {
                return Err(Error::agent_purged("This agent was permanently purged."));
            }
            None => return Err(Error::unknown_agent("No agent has this ID.")),
        };
        super::decode_agent(&record_text(&data, columns)?)
    }
    pub fn agent_page(
        &self,
        after: u64,
        limit: u64,
        parent: Option<&str>,
        auth: Option<&str>,
    ) -> Result<Value> {
        api::pagination::rows(limit)?;
        self.sql().cached().with_rows(
            shared_queries::AGENT_PAGE,
            params![
                convert::capped_i64(after),
                parent,
                convert::capped_i64(limit),
                auth
            ],
            |rows| {
                let mut page = Vec::new();
                let mut cursor = None;
                let mut budget = super::PageBudget::new();
                while let Some(row) = rows.next::<rows::AgentPageRow>()? {
                    let agent = super::decode_agent(&row.record)?;
                    let idle_since = self.idle_since(&agent.id)?;
                    let value = api::listed_agent(&agent, idle_since, row.approval_id.as_deref());
                    #[cfg(not(feature = "comb"))]
                    let value = {
                        let mut value = value;
                        if let Some(hold) = self.agent_hold(&agent.id)? {
                            value["held_for_build"] = api::value(hold);
                        }
                        value
                    };
                    let size = value.to_string().len();
                    if !budget.accept(size)? {
                        break;
                    }
                    cursor = Some(row.sequence);
                    page.push(value);
                }
                Ok(api::value(api::AgentsPage {
                    agents: page,
                    next_after: cursor,
                }))
            },
        )
    }
    pub fn has_ready(&self) -> Result<bool> {
        self.query_row_cached(shared_queries::HAS_READY_AGENT, [], |r| r.get(0))
    }
}

impl Store {
    pub fn recover_agents(&self) -> Result<()> {
        self.sql().execute_batch(queries::RECOVER_AGENTS)?;
        self.sql()
            .execute_batch(&schema::queries::refresh_mail("agents.id"))?;
        Ok(())
    }
    pub fn fence_provider(&self, provider: &str) -> Result<()> {
        self.execute_cached(queries::FENCE_PROVIDER, [provider])?;
        self.wake_all.set(true);
        Ok(())
    }
    pub fn unblock_agent(&self, id: &str) -> Result<()> {
        self.execute_cached(queries::UNBLOCK_AGENT, [id])?;
        self.changed(id);
        Ok(())
    }
    /// Caller owns the hub transaction and the current execution's agent row.
    pub fn record_conversation_release(&self, agent: &mut Agent, confirmed: bool) -> Result<()> {
        let current = self.agent(&agent.id)?;
        if current.execution_id != agent.execution_id
            || current.provider_id() != agent.provider_id()
        {
            return Err(Error::stale_execution(
                "Release no longer owns this provider execution.",
            ));
        }
        agent.conversation_clean = confirmed;
        if confirmed
            && current.recovery_fence
                == Some(crate::runtime::domain::RecoveryFence::ExecutionCleanupUncertain)
        {
            agent.recovery_fence = None;
            self.unblock_agent(&agent.id)?;
        }
        self.save(agent)?;
        if current.branch_hold > 0 || current.branch_unbound() {
            self.deadlines_changed();
        }
        Ok(())
    }
    pub fn claim(&self, class: usize) -> Result<Option<String>> {
        // Admission has exactly four classes; an invalid class must not select work.
        if class > crate::runtime::limits::MAX_READY_CLASS {
            return Err(numeric_range());
        }
        let class = i64::try_from(class).map_err(numeric)?;
        let id: Option<String> =
            self.sql()
                .cached()
                .query_opt(queries::NEXT_READY_AGENT, [class], |r| r.get(0))?;
        if let Some(id) = &id {
            self.execute_cached(queries::CLAIM_AGENT, [id])?;
        }
        Ok(id)
    }
    pub fn release_claim(&self, id: &str) -> Result<()> {
        let branch = self.query_opt_cached(
            queries::RELEASE_AGENT_CLAIM,
            [id],
            rows::ReleasedClaim::read,
        )?;
        self.changed(id);
        if let Some(rows::ReleasedClaim { hold, phase }) = branch {
            if hold > 0 {
                self.deadlines_changed();
            }
            if matches!(phase.as_deref(), Some("failed" | "uncertain")) {
                self.branch_execution_released(id)?;
            }
        }
        Ok(())
    }
    pub fn save(&self, agent: &Agent) -> Result<()> {
        self.save_at(agent, state::now())
    }
    /// The database owns the first permanent-ending stamp; stale copies cannot reset it.
    #[expect(
        clippy::expect_used,
        reason = "Agent fields serialize to a JSON object with string field names."
    )]
    pub(in crate::runtime) fn save_at(&self, agent: &Agent, now: u64) -> Result<()> {
        let mut value = serde_json::to_value(agent).expect("agent fields serialize to JSON");
        let object = value
            .as_object_mut()
            .expect("saved agent record is an object");
        for name in schema::REMOVED_FIELDS {
            object.remove(*name);
        }
        let mut values = vec![rusqlite::types::Value::Text(agent.id.clone())];
        let mut hot = Vec::new();
        for name in schema::HOT {
            let value = object.remove(*name).unwrap_or(Value::Null);
            hot.push(match value {
                Value::Null => rusqlite::types::Value::Null,
                Value::Bool(b) => rusqlite::types::Value::Integer(i64::from(b)),
                Value::Number(n) => {
                    rusqlite::types::Value::Integer(n.as_i64().ok_or_else(numeric_range)?)
                }
                Value::String(s) => rusqlite::types::Value::Text(s),
                _ => return Err(Error::database_error("Invalid hot field.")),
            });
        }
        values.push(rusqlite::types::Value::Text(value.to_string()));
        values.extend(hot);
        values.push(if agent.terminal() {
            rusqlite::types::Value::Integer(i64::try_from(now).map_err(|cause| {
                Error::database_error("Agent ending timestamp exceeds SQLite range.")
                    .with_source(cause)
            })?)
        } else {
            rusqlite::types::Value::Null
        });
        let process_options = agent
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
            .transpose()?
            .filter(|key| key != &process_options);
        values.push(process_options.into());
        values.push(reserved.map_or(rusqlite::types::Value::Null, Into::into));
        self.sql()
            .cached()
            .execute(&queries::SAVE_AGENT, rusqlite::params_from_iter(values))?;
        if !agent.instructions.is_empty() {
            self.execute_cached(queries::INSERT_INSTRUCTION_SNAPSHOT, [&agent.instructions])?;
            self.execute_cached(
                queries::BIND_AGENT_INSTRUCTIONS,
                params![agent.id, agent.instructions],
            )?;
        }
        self.changed(&agent.id);
        Ok(())
    }
}
