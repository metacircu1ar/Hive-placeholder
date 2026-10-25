//! Read event pages and durably publish agent and provider events.
use super::{
    Agent, EventKind, Queries, Store, api, numeric_range, queries, queries as shared_queries, rows,
};
use crate::{
    convert,
    error::{Error, Result},
    runtime::domain::ProviderEventKind,
};
use rusqlite::params;
use serde_json::Value;
// Move larger event payloads to artifacts instead of storing them inline in the event row.
pub(super) const INLINE_EVENT_BYTES: usize = 16384;
// Skip artifact fields larger than this when retaining inline metadata.
const INDEXED_FIELD_BYTES: usize = 1024;
// Skip fields that would push inline artifact metadata beyond this byte budget.
const INDEXED_EVENT_BYTES: usize = 8192;
// Per-field allowance for JSON structure when accounting inline artifact metadata.
const INDEXED_FIELD_OVERHEAD: usize = 8;

impl Queries {
    pub fn events(&self, id: &str, after: u64, limit: u64) -> Result<Value> {
        if let Some(id) = id.strip_prefix("artifact:") {
            return self.output(
                id,
                after,
                usize::try_from(api::pagination::bytes(limit)?).map_err(super::numeric)?,
            );
        }
        api::pagination::rows(limit)?;
        self.agent(id)?;
        self.sql().cached().with_rows(
            shared_queries::EVENT_PAGE,
            params![id, convert::capped_i64(after), convert::capped_i64(limit)],
            |rows| {
                let pruned = self
                    .query_opt_cached(shared_queries::PRUNED_EVENT_CURSOR, [id], |r| {
                        rows::unsigned(r, 0)
                    })?
                    .unwrap_or(0);
                let mut result = Vec::new();
                let mut cursor = after.max(pruned);
                let mut budget = super::PageBudget::new();
                while let Some(row) = rows.next::<rows::EventRow>()? {
                    let (seq, value) = event_record(id, row)?;
                    if !budget.accept(value.to_string().len())? {
                        break;
                    }
                    cursor = cursor.max(seq);
                    result.push(value);
                }
                Ok(api::value(api::EventsPage {
                    events: result,
                    cursor,
                    pruned_through: pruned,
                    history_expired: after < pruned,
                }))
            },
        )
    }
    /// Retained events before `before`, oldest first. The byte budget runs from the
    /// newest row; `older` is set while older retained rows remain.
    pub fn events_before(&self, id: &str, before: u64, limit: u64) -> Result<Value> {
        api::pagination::rows(limit)?;
        self.agent(id)?;
        let pruned = self
            .query_opt_cached(shared_queries::PRUNED_EVENT_CURSOR, [id], |r| {
                rows::unsigned(r, 0)
            })?
            .unwrap_or(0);
        self.sql().cached().with_rows(
            shared_queries::EVENT_PAGE_BEFORE,
            params![
                id,
                convert::capped_i64(before),
                convert::capped_i64(pruned),
                convert::capped_i64(limit.saturating_add(1))
            ],
            |rows| {
                let mut result = Vec::new();
                let mut oldest = None;
                let mut more = false;
                let mut budget = super::PageBudget::new();
                while let Some(row) = rows.next::<rows::EventRow>()? {
                    let (seq, value) = event_record(id, row)?;
                    if result.len() as u64 == limit || !budget.accept(value.to_string().len())? {
                        more = true;
                        break;
                    }
                    oldest = Some(seq);
                    result.push(value);
                }
                result.reverse();
                let older = oldest.filter(|_| more);
                Ok(api::value(api::HistoryPage {
                    events: result,
                    older,
                    pruned_through: pruned,
                    history_expired: older.is_none() && pruned > 0,
                }))
            },
        )
    }
}

/// One saved event row as its public record, with its sequence.
fn event_record(id: &str, row: rows::EventRow) -> Result<(u64, Value)> {
    let seq = convert::unsigned_u64(row.sequence).map_err(super::numeric)?;
    let value = api::value(api::EventRecord {
        seq: row.sequence,
        agent_id: id,
        execution_id: row.execution,
        timestamp: row.timestamp,
        kind: row.kind,
        data: serde_json::from_str::<Value>(&row.data).map_err(|error| {
            Error::database_error("Invalid saved event data.").with_source(error)
        })?,
    });
    Ok((seq, value))
}

impl Store {
    pub fn provider_event(
        &self,
        provider: &str,
        kind: ProviderEventKind,
        data: Value,
    ) -> Result<()> {
        self.execute_cached(
            queries::INSERT_PROVIDER_EVENT,
            params![provider, super::convert::now()?, kind, data.to_string()],
        )?;
        self.provider_notice(provider, kind);
        Ok(())
    }
    pub fn event(&self, agent: &Agent, kind: EventKind, data: Value) -> Result<()> {
        self.record_event(agent, kind, data)?;
        Ok(())
    }
    pub(crate) fn turn_started(
        &self,
        agent: &Agent,
        data: api::events::TurnStarted<'_>,
    ) -> Result<()> {
        self.execute_cached(
            queries::BIND_DELIVERY_TURN,
            params![
                data.turn_id,
                agent.execution_id,
                agent.provider_id(),
                agent.thread_id,
                data.message_id
            ],
        )?;
        self.record_event(agent, EventKind::TurnStarted, api::value(&data))?;
        // Event payloads retain the persisted sender spelling (human or an agent ID).
        if data.sender == crate::runtime::hub::Caller::Human.name() || self.joins_possible.get() {
            self.execute_cached(queries::BIND_CALL_START, [data.message_id])?;
        }
        Ok(())
    }
    #[cfg(feature = "comb")]
    pub(super) fn record_event(
        &self,
        agent: &Agent,
        kind: EventKind,
        data: Value,
    ) -> Result<String> {
        let evidence = self
            .collectors_possible
            .get()
            .then(|| super::collectors::capture::automatic(agent, kind, &data))
            .flatten();
        if let Some((source, evidence)) = evidence {
            self.atomic(|store| {
                let encoded = store.record_event_inner(agent, kind, data)?;
                store.collect_evidence(
                    store.connection.last_insert_rowid(),
                    &agent.id,
                    source,
                    &evidence,
                )?;
                Ok(encoded)
            })
        } else {
            self.record_event_inner(agent, kind, data)
        }
    }
    #[cfg(not(feature = "comb"))]
    pub(super) fn record_event(
        &self,
        agent: &Agent,
        kind: EventKind,
        data: Value,
    ) -> Result<String> {
        self.record_event_inner(agent, kind, data)
    }
    #[expect(
        clippy::expect_used,
        reason = "A serde_json::Value serializes infallibly to UTF-8 JSON bytes."
    )]
    fn record_event_inner(&self, agent: &Agent, kind: EventKind, data: Value) -> Result<String> {
        self.observed(&agent.id);
        // A child's spawn changes its parent's view (child count and markers) without
        // any event in the parent's own history.
        if kind == EventKind::Spawned
            && let Some(parent) = &agent.parent_id
        {
            self.observed(parent);
        }
        // Pending approvals are tree-row state that is not part of the agent row.
        if matches!(
            kind,
            EventKind::ApprovalRequested | EventKind::ApprovalClosed
        ) {
            self.tree_changed.set(true);
        }
        let encoded = serde_json::to_vec(&data).expect("event JSON value serializes");
        let data = if encoded.len() > INLINE_EVENT_BYTES {
            let (id, bytes) = self.publish_artifact(&data, encoded.len(), Some(&agent.id), None)?;
            let mut indexed = indexed_fields(&data)?;
            indexed.insert(
                "artifact".into(),
                api::value(api::events::ArtifactReference {
                    id: &id,
                    bytes,
                    format: "application/json",
                    read_target: format!("artifact:{id}"),
                    read: concat!(
                        "Use hive_read(target=read_target, after=byte offset); follow ",
                        "next_offset until eof. CLI: output <id> --offset <byte offset>."
                    ),
                }),
            );
            Value::Object(indexed).to_string()
        } else {
            String::from_utf8(encoded).expect("serialized event JSON is UTF-8")
        };
        self.execute_cached(
            queries::INSERT_EVENT,
            params![
                agent.id,
                agent.execution_id,
                super::convert::now()?,
                kind,
                data
            ],
        )?;
        self.agent_notice(agent, kind);
        Ok(data)
    }
}

fn indexed_fields(data: &Value) -> Result<serde_json::Map<String, Value>> {
    let mut indexed = serde_json::Map::new();
    if let Some(fields) = data.as_object() {
        let mut retained = 0;
        for (key, value) in fields {
            let size = key
                .len()
                .checked_add(value.to_string().len())
                .and_then(|size| size.checked_add(INDEXED_FIELD_OVERHEAD))
                .ok_or_else(numeric_range)?;
            if size <= INDEXED_FIELD_BYTES && retained + size <= INDEXED_EVENT_BYTES {
                indexed.insert(key.clone(), value.clone());
                retained += size;
            }
        }
    }
    Ok(indexed)
}
