mod queries;
use super::{BTreeMap, CollectionSource, Error, Result, Store, key, params, sources};
use crate::runtime::{
    collectors::{Admission, Disposition, Evidence},
    domain::{Agent, EventKind},
    hub::Caller,
};
use serde_json::{Value, json};

const MAX_ITEMS: i64 = 1024;
const MAX_BYTES: i64 = 16 * 1024 * 1024;

pub(in crate::runtime::store) fn automatic(
    agent: &Agent,
    kind: EventKind,
    data: &Value,
) -> Option<(CollectionSource, Value)> {
    let (source, outcome) = automatic_outcome(kind, data)?;
    Some(automatic_evidence(agent, kind, data, source, outcome))
}

fn automatic_outcome(kind: EventKind, data: &Value) -> Option<(CollectionSource, Value)> {
    Some(match kind {
        EventKind::TaskCompleted => (
            CollectionSource::TaskCompleted,
            crate::runtime::api::value(Outcome {
                outcome: "completed",
                error_code: None,
            }),
        ),
        EventKind::ExecutionFailed => {
            let code: crate::error::ErrorCode = crate::observability::optional(
                serde_json::from_value(data.get("error")?.get("code")?.clone()),
                "collector_error_projection",
            )?;
            (
                CollectionSource::TurnFailed,
                crate::runtime::api::value(Outcome {
                    outcome: "failed",
                    error_code: Some(code),
                }),
            )
        }
        EventKind::TurnFinished => {
            let status = data.get("status")?.as_str()?;
            if !matches!(status, "failed" | "interrupted") {
                return None;
            }
            (
                CollectionSource::TurnFailed,
                crate::runtime::api::value(Outcome {
                    outcome: status,
                    error_code: None,
                }),
            )
        }
        EventKind::ApprovalClosed => {
            let action: crate::approvals::ApprovalAction = crate::observability::optional(
                serde_json::from_value(data.get("action")?.clone()),
                "collector_approval_projection",
            )?;
            (
                CollectionSource::ApprovalAnswer,
                crate::runtime::api::value(Approval { action }),
            )
        }
        _ => return None,
    })
}

fn automatic_evidence(
    agent: &Agent,
    kind: EventKind,
    data: &Value,
    source: CollectionSource,
    outcome: Value,
) -> (CollectionSource, Value) {
    let turn = if matches!(
        kind,
        EventKind::TurnFinished | EventKind::ExecutionFailed | EventKind::TaskCompleted
    ) {
        data.get("turn_id").and_then(Value::as_str)
    } else {
        agent.turn_id.as_deref()
    };
    if kind == EventKind::ApprovalClosed {
        return (
            source,
            json!({"source":source,
                "actor":"human",
                "subject_agent":agent.id,
                "subject_role":agent.role,

            "execution":data.get("origin").and_then(|o| o.get("execution")).and_then(Value::as_str),
            "turn":data.get("origin").and_then(|o| o.get("turn")).and_then(Value::as_str),
            "approval_id":data.get("approval_id").and_then(Value::as_str),"evidence":outcome}),
        );
    }
    (
        source,
        json!({"source":source,
            "actor":agent.id,
            "role":agent.role,
            "execution":agent.execution_id,
            "turn":turn,
            "evidence":outcome}),
    )
}
impl Store {
    pub(in crate::runtime) fn submit_evidence(
        &self,
        caller: &Caller,
        evidence: &Evidence,
    ) -> Result<Admission> {
        let source = evidence.validate(caller.is_human())?;
        let (role, execution, turn) = if caller.is_human() {
            (None, None, None)
        } else {
            let agent = caller.checked_agent(self)?;
            if agent.terminal() || agent.control.is_some() || agent.turn_id.is_none() {
                return Err(Error::stale_execution(
                    "Evidence requires an active agent turn.",
                ));
            }
            (agent.role, agent.execution_id, agent.turn_id)
        };
        self.atomic(|store| {
            let data = json!({"source":source,
                "actor":caller.name(),
                "role":role,
                "execution":execution,
                "turn":turn,
                "evidence":evidence});
            store.execute_cached(
                super::super::queries::INSERT_EVENT,
                params![
                    caller.name(),
                    execution,
                    super::super::convert::now()?,
                    EventKind::EvidenceSubmitted,
                    data.to_string()
                ],
            )?;
            let seq = store.connection.last_insert_rowid();
            let destinations = store.collect_evidence(seq, caller.name(), source, &data)?;
            Ok(Admission {
                event: seq,
                destinations,
            })
        })
    }
    pub(in crate::runtime::store) fn collect_evidence(
        &self,
        seq: i64,
        actor: &str,
        source: CollectionSource,
        data: &Value,
    ) -> Result<BTreeMap<String, Disposition>> {
        let destinations =
            self.sql()
                .cached()
                .query_map(queries::COLLECT_EVIDENCE_SELECT, [], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, bool>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })?;
        let installation: String =
            self.sql()
                .cached()
                .query_row(queries::COLLECT_EVIDENCE_SELECT_2, [], |r| r.get(0))?;
        let body = evidence_body(&installation, seq, data).to_string();
        let mut result = BTreeMap::new();
        for (name, enabled, selected) in destinations {
            let disposition = if !enabled {
                Disposition::Disabled
            } else if !sources(&selected)?.contains(&source) {
                Disposition::NotSelected
            } else {
                self.collector_gap(&name)?;
                if self.collector_capacity(&name, body.len())? {
                    self.insert_collection(&name, seq, &key((&installation, &name, seq))?, &body)?;
                    Disposition::Queued
                } else {
                    let count: i64 = self.sql().cached().query_row(
                        queries::COLLECT_EVIDENCE_SELECT_3,
                        [&name],
                        |r| r.get(0),
                    )?;
                    self.execute_cached(queries::COLLECT_EVIDENCE_UPDATE, params![name, seq])?;
                    if count == 0 {
                        self.execute_cached(
                            super::super::queries::INSERT_EVENT,
                            params![
                                actor,
                                Option::<String>::None,
                                super::super::convert::now()?,
                                EventKind::CollectorOverflow,
                                json!({"destination":name,"first_skipped_event":seq}).to_string()
                            ],
                        )?;
                    }
                    Disposition::Full
                }
            };
            result.insert(name, disposition);
        }
        Ok(result)
    }
    fn collector_capacity(&self, destination: &str, bytes: usize) -> Result<bool> {
        self.sql().cached().query_row(
            queries::COLLECTOR_CAPACITY_SELECT,
            params![
                destination,
                MAX_ITEMS,
                i64::try_from(bytes).map_err(super::super::numeric)?,
                MAX_BYTES
            ],
            |r| r.get(0),
        )
    }
    fn insert_collection(&self, destination: &str, seq: i64, id: &str, body: &str) -> Result<()> {
        self.execute_cached(
            queries::INSERT_COLLECTION_INSERT,
            params![
                id,
                destination,
                seq,
                body,
                i64::try_from(body.len()).map_err(super::super::numeric)?
            ],
        )?;
        Ok(())
    }
    pub(super) fn collector_gap(&self, destination: &str) -> Result<()> {
        let (count, first, last, pending, bytes): (i64, Option<i64>, Option<i64>, i64, i64) = self
            .sql()
            .cached()
            .query_row(queries::COLLECTOR_GAP_SELECT, [destination], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?;
        if count == 0 {
            return Ok(());
        }
        let installation: String =
            self.sql()
                .cached()
                .query_row(queries::COLLECTOR_GAP_SELECT_2, [], |r| r.get(0))?;
        let body = gap_body(&installation, destination, count, first, last).to_string();
        if pending >= MAX_ITEMS
            || bytes + i64::try_from(body.len()).map_err(super::super::numeric)? > MAX_BYTES
        {
            return Ok(());
        }
        let id = key((&installation, destination, "gap", first, last))?;
        self.insert_collection(destination, last.unwrap_or(0), &id, &body)?;
        self.execute_cached(queries::COLLECTOR_GAP_UPDATE, [destination])?;
        Ok(())
    }
}

fn evidence_body(installation: &str, seq: i64, data: &Value) -> Value {
    json!({"format":crate::compatibility::COLLECTOR.current,
        "installation":installation,
        "event":seq,
        "record":data})
}
fn gap_body(
    installation: &str,
    destination: &str,
    count: i64,
    first: Option<i64>,
    last: Option<i64>,
) -> Value {
    json!({"format":crate::compatibility::COLLECTOR.current,
        "installation":installation,
        "kind":"collection_gap",
        "destination":destination,
        "skipped_count":count,
        "first_event":first,
        "last_event":last})
}

#[cfg(test)]
#[test]
fn compatibility_collector_envelopes_keep_queued_bytes() {
    let installation = "12345678-1234-4234-8234-123456789abc";
    crate::compatibility::contract(
        "collector-envelopes.json",
        &[
            evidence_body(
                installation,
                3,
                &json!({"source":"task_completed",
                    "actor":"agent",
                    "role":"writer",
                    "execution":"execution",
                    "turn":"turn",
                    "evidence":{"outcome":"completed"}}),
            ),
            gap_body(installation, "knowledge", 2, Some(4), Some(5)),
        ],
    );
}

#[derive(serde::Serialize)]
struct Outcome<'a> {
    outcome: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_code: Option<crate::error::ErrorCode>,
}
#[derive(serde::Serialize)]
struct Approval {
    action: crate::approvals::ApprovalAction,
}
