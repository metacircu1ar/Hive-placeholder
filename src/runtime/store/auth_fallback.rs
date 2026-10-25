//! Persisted login availability, continuation origins and pending auth work.
mod queries;

use super::{Queries, Store, api};
use crate::{
    error::{Error, Result},
    runtime::auth_fallback::Availability,
};
use rusqlite::params;

impl Queries {
    pub(crate) fn continuation_origin(
        &self,
        recipient: &str,
        message: &str,
    ) -> Result<ContinuationOrigin> {
        let (kind, root, attempt): (
            Option<crate::runtime::domain::MessageKind>,
            Option<String>,
            Option<i64>,
        ) = self.sql().cached().query_row(
            queries::CONTINUATION_ORIGIN,
            params![recipient, message],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        if kind == Some(crate::runtime::domain::MessageKind::Continuation) {
            Ok(ContinuationOrigin {
                root: root.ok_or_else(|| Error::database_error("Missing continuation root."))?,
                attempt: attempt
                    .and_then(|n| u64::try_from(n).ok())
                    .and_then(|n| n.checked_add(1))
                    .filter(|n| i64::try_from(*n).is_ok())
                    .ok_or_else(|| Error::database_error("Invalid continuation attempt."))?,
            })
        } else {
            Ok(ContinuationOrigin {
                root: message.to_owned(),
                attempt: 1,
            })
        }
    }
    /// Bound agents whose login needs a boundary. Balanced-group agents qualify
    /// only with deliverable next-turn input; idle ones keep their binding.
    pub(crate) fn affected_auth_agents(&self, balanced: &str) -> Result<Vec<String>> {
        self.sql()
            .cached()
            .query_column(queries::AFFECTED_AUTH_AGENTS, [balanced])
    }
    /// Accepted next-turn input, including continuations; scheduled input
    /// counts only once promoted.
    pub(crate) fn deliverable_input(&self, agent: &str) -> Result<bool> {
        self.query_row_cached(queries::DELIVERABLE_INPUT, [agent], |r| r.get(0))
    }
    pub(crate) fn auth_switches_after(&self, after: &str) -> Result<Vec<String>> {
        self.sql()
            .cached()
            .query_column(queries::AUTH_SWITCHES_AFTER, [after])
    }
    pub(crate) fn next_deadline(&self) -> Result<Option<u64>> {
        let value: Option<i64> = self
            .sql()
            .query_row(queries::NEXT_LOGIN_DEADLINE, [], |r| r.get(0))?;
        let scheduled: Option<i64> =
            self.query_row_cached(super::scheduled::queries::NEXT, [], |r| r.get(0))?;
        let branch: Option<i64> =
            self.query_row_cached(queries::NEXT_BRANCH_DEADLINE, [], |r| r.get(0))?;
        value
            .into_iter()
            .chain(
                self.next_join_deadline()?
                    .map(crate::convert::sqlite_i64)
                    .transpose()
                    .map_err(super::numeric)?,
            )
            .chain(branch)
            .chain(scheduled)
            .min()
            .map(|at| {
                u64::try_from(at).map_err(|cause| {
                    Error::database_error("Invalid runtime deadline.").with_source(cause)
                })
            })
            .transpose()
    }
    pub(crate) fn quota_waiters(&self, groups: &str) -> Result<Vec<String>> {
        self.sql().cached().query_map(
            queries::QUOTA_WAITERS,
            params![stamp(crate::state::now())?, groups],
            |r| r.get(0),
        )
    }
    pub(crate) fn probe_input(
        &self,
        agent: &str,
        message: &str,
    ) -> Result<Option<crate::runtime::domain::MessageState>> {
        self.sql()
            .cached()
            .query_opt(queries::PROBE_INPUT, params![agent, message], |r| r.get(0))
    }
    pub(crate) fn probe_candidates(&self, now: u64) -> Result<Vec<Availability>> {
        let rows: Vec<super::rows::LoginGeneration> = self
            .sql()
            .cached()
            .query_rows(queries::PROBE_CANDIDATES, [stamp(now)?])?;
        rows.into_iter()
            .map(
                |super::rows::LoginGeneration {
                     profile,
                     generation,
                 }| {
                    self.availability(
                        &profile,
                        uuid::Uuid::parse_str(&generation).map_err(|e| {
                            Error::database_error("Invalid probe generation.").with_source(e)
                        })?,
                    )
                },
            )
            .collect()
    }
    pub(crate) fn cutoff_agents(&self, after: &str) -> Result<Vec<String>> {
        self.sql()
            .cached()
            .query_column(queries::CUTOFF_AGENTS, [after])
    }

    pub(crate) fn auth_status(&self, auth: &str) -> Result<api::AuthAvailability> {
        let generations: Vec<String> = self
            .sql()
            .cached()
            .query_column(queries::AUTH_GENERATIONS, [auth])?;
        let values = generations
            .into_iter()
            .map(|generation| {
                let generation = uuid::Uuid::parse_str(&generation).map_err(|cause| {
                    Error::database_error("Invalid login generation.").with_source(cause)
                })?;
                self.availability(auth, generation)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(api::AuthAvailability {
            availability: values,
        })
    }

    pub(crate) fn auth_groups<'a>(
        &self,
        config: &'a crate::config::Config,
    ) -> Result<api::AuthGroups<'a>> {
        // Each member requires ordered SQL and config reads; finish those before
        // reading the group's provider so failures retain their exact precedence.
        let mut groups = Vec::new();
        for (name, group) in &config.auth_groups {
            let member = |profile, weight| -> Result<_> {
                let load = self.auth_load(profile)?;
                Ok(api::AuthMember {
                    profile,
                    method: config.profile(profile)?.method,
                    weight,
                    availability: self.auth_status(profile)?.availability,
                    bound_agents: load.bound_agents,
                    active_turns: load.active_turns,
                })
            };
            let members = group
                .members
                .iter()
                .map(|m| member(m.profile(), group.balanced().then(|| m.weight())))
                .collect::<Result<_>>()?;
            let paid_tail = group
                .paid_tail
                .iter()
                .map(|profile| member(profile, None))
                .collect::<Result<_>>()?;
            groups.push(api::AuthGroup {
                name,
                provider: config.auth_provider(name)?,
                mode: group.mode,
                members,
                paid_tail,
            });
        }
        Ok(api::AuthGroups { groups })
    }
    pub(crate) fn restoration_profiles(&self) -> Result<Vec<String>> {
        self.sql()
            .cached()
            .query_column(queries::RESTORATION_PROFILES, [])
    }
    pub(crate) fn cutoff_input(
        &self,
        agent: &crate::runtime::domain::Agent,
        turn: &str,
    ) -> Result<Option<String>> {
        self.sql().cached().query_opt(
            queries::CUTOFF_INPUT,
            params![agent.id, agent.execution_id, turn],
            |r| r.get(0),
        )
    }
    pub(crate) fn auth_handoff(
        &self,
        agent: &str,
        reason: crate::runtime::auth_fallback::AuthReason,
    ) -> Result<String> {
        // Retain a bounded login-switch handoff even when several sections are full.
        const LIMIT: usize = 32 * 1024;
        // Each section has its own byte and row budget before the overall cap.
        const SECTION_BYTES: usize = 4800;
        const SECTION_ROWS: usize = 32;
        let reason = reason.as_str();
        let mut out = super::handoff::Handoff::new(format!(
            concat!(
                "Hive login handoff. Reason: {reason}. Your identity and mailbox ",
                "are unchanged. This is historical context, not a new incoming ",
                "message.\n",
            ),
            reason = reason,
        ));
        let queries = [
            ("Recent output/events", queries::HANDOFF_EVENTS),
            ("Recent inputs", queries::HANDOFF_INPUTS),
            ("Original task", queries::HANDOFF_ORIGINAL_TASK),
            ("Children", queries::HANDOFF_CHILDREN),
            (
                "Unanswered outgoing messages",
                queries::HANDOFF_OUTGOING_MESSAGES,
            ),
            ("Pending human approval", queries::HANDOFF_PENDING_APPROVAL),
        ];
        // Reserve a share for open state/original task; newest rows win within each share.
        for (title, sql) in queries {
            out.section(title);
            self.sql().cached().with_rows(sql, [agent], |rows| {
                let remaining = out.remaining(LIMIT).min(SECTION_BYTES);
                out.rows(rows, remaining, SECTION_ROWS)
            })?;
        }
        Ok(out.finish())
    }
    pub(crate) fn auth_revision(&self) -> Result<u64> {
        let revision: i64 = self
            .sql()
            .query_row(queries::AUTH_REVISION, [], |r| r.get(0))?;
        u64::try_from(revision).map_err(|cause| {
            Error::database_error("Invalid availability revision.").with_source(cause)
        })
    }

    pub(crate) fn availability(
        &self,
        profile: &str,
        generation: uuid::Uuid,
    ) -> Result<Availability> {
        let record: Option<String> = self.sql().cached().query_opt(
            queries::AVAILABILITY,
            params![profile, generation.to_string()],
            |r| r.get(0),
        )?;
        record.map_or_else(
            || Ok(Availability::available(profile, generation)),
            |record| {
                crate::runtime::domain::decode::from_str(&record).map_err(|cause| {
                    Error::database_error("Invalid login availability.").with_source(cause)
                })
            },
        )
    }
}
pub(crate) struct ContinuationOrigin {
    pub root: String,
    pub attempt: u64,
}
impl Store {
    pub(crate) fn skipped_members(&self, agent: &super::Agent, skipped: &[String]) -> Result<()> {
        for auth in skipped {
            self.event(
                agent,
                crate::runtime::domain::EventKind::MemberSkipped,
                api::value(api::events::MemberSkipped {
                    auth,
                    reason: "model_unavailable",
                }),
            )?;
        }
        Ok(())
    }

    pub(crate) fn enqueue_continuation(
        &self,
        id: &str,
        recipient: &str,
        body: &str,
        root: &str,
        attempt: u64,
    ) -> Result<bool> {
        let added = self.execute_cached(
            queries::INSERT_CONTINUATION,
            params![id, recipient, body, root, stamp(attempt)?],
        )?;
        if added > 0 {
            self.execute_cached(queries::INSERT_CONTINUATION_DELIVERY, [id])?;
            self.changed(recipient);
        }
        if added == 0 {
            let matches: bool = self.query_row_cached(
                queries::CONTINUATION_MATCHES,
                params![id, recipient, root, stamp(attempt)?],
                |r| r.get(0),
            )?;
            if !matches {
                return Err(Error::database_error("Conflicting continuation identity."));
            }
        }
        Ok(added > 0)
    }

    pub(crate) fn ensure_availability(&self, profile: &str, generation: uuid::Uuid) -> Result<()> {
        if self.availability(profile, generation)?.revision == 0 {
            let mut value = Availability::available(profile, generation);
            self.save_revised(&mut value)?;
        }
        Ok(())
    }
    pub(crate) fn select_key_authority(
        &self,
        profile: &str,
        generation: uuid::Uuid,
        key: Option<&crate::state::KeyAuthority>,
    ) -> Result<()> {
        let mut value = self.availability(profile, generation)?;
        if value.key.as_ref() == key {
            if value.revision == 0 {
                self.save_revised(&mut value)?;
            }
            return Ok(());
        }
        let Some(key) = key else {
            return Err(Error::state_inconsistent(
                "API key authority cannot become a subscription.",
            ));
        };
        if value
            .key
            .as_ref()
            .is_some_and(|old| old.revision >= key.revision)
        {
            return Err(Error::agent_busy("API key proof was superseded; retry."));
        }
        let previous = value.key.replace(key.clone());
        value.mark_available();
        value.clear_probe();
        value.confirmed_at = None;
        value.updated_at = crate::state::now();
        self.save_revised(&mut value)?;
        if previous.is_some() {
            self.provider_event(
                &format!("auth:{profile}"),
                crate::runtime::domain::ProviderEventKind::ApiKeyRotated,
                api::value(api::events::ApiKeyRotated {
                    profile,
                    generation,
                    previous: previous.as_ref(),
                    selected: key,
                }),
            )?;
        }
        Ok(())
    }

    pub(crate) fn expire_login_windows(&self, now: u64) -> Result<usize> {
        let due: Vec<super::rows::LoginGeneration> = self
            .sql()
            .cached()
            .query_rows(queries::EXPIRED_LOGIN_WINDOWS, [stamp(now)?])?;
        let count = due.len();
        for super::rows::LoginGeneration {
            profile,
            generation,
        } in due
        {
            let generation = uuid::Uuid::parse_str(&generation).map_err(|cause| {
                Error::database_error("Invalid login generation.").with_source(cause)
            })?;
            let mut value = self.availability(&profile, generation)?;
            value.state = crate::runtime::auth_fallback::AvailabilityState::Available;
            if value.until.is_some() {
                value.retry_at = None;
            }
            value.until = None;
            value.confirmed_at = None;
            value.updated_at = now;
            self.save_revised(&mut value)?;
        }
        Ok(count)
    }

    pub(crate) fn save_revised(&self, value: &mut Availability) -> Result<()> {
        value.revision = self.auth_revision()? + 1;
        self.save_availability(value)
    }
    pub(crate) fn save_availability(&self, value: &Availability) -> Result<()> {
        self.execute_cached(
            queries::SAVE_AVAILABILITY,
            params![
                value.profile,
                value.generation.to_string(),
                stamp(value.revision)?,
                value.state,
                value.until.map(stamp).transpose()?,
                value.retry_at.map(stamp).transpose()?,
                value.probe_id(),
                value.probe_agent(),
                value.probe_message(),
                value.probe_deadline().map(stamp).transpose()?,
                value.probe_error(),
                value.window,
                value.reason,
                value.confirmed_at.map(stamp).transpose()?,
                stamp(value.updated_at)?,
                value.key.as_ref().map(|key| api::value(key).to_string())
            ],
        )?;
        self.deadlines_changed();
        Ok(())
    }
}

fn stamp(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|cause| {
        Error::database_error("Availability timestamp or revision exceeds SQLite range.")
            .with_source(cause)
    })
}

#[cfg(test)]
#[test]
fn out_of_range_stamp_keeps_numeric_cause_and_public_error() {
    use std::error::Error as _;
    let error = stamp(u64::MAX).expect_err("timestamp exceeds SQLite");
    assert_eq!(error.code(), "database_error");
    assert_eq!(
        error.message(),
        "Availability timestamp or revision exceeds SQLite range."
    );
    assert!(
        error
            .source()
            .is_some_and(<dyn std::error::Error>::is::<std::num::TryFromIntError>)
    );
}
