//! Coordinate auth deadlines and retain bounded native discovery jobs through completion.
use super::AuthReason;
use super::WaitingReason;
#[cfg(test)]
use super::prepare;
#[cfg(test)]
use super::switch::apply_switch;
use super::{restore::restore_profile, switch::finish_switch};
use crate::runtime::api::{self, events};
#[cfg(test)]
use crate::runtime::domain::AgentState;
use crate::{
    error::{Error, Result},
    runtime::{Core, Shared, domain::EventKind, provider::Pool},
};
#[cfg(test)]
use serde_json::json;
use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

// Four discovery jobs bound parallel native work without blocking the deadline owner.
const JOB_LIMIT: usize = 4;
// Saturate exponential retry growth after 2^9 seconds, before the five-minute cap.
const MAX_RETRY_EXPONENT: u32 = 9;
const MAX_RETRY_SECONDS: u64 = 300;
// A full SQL deadline page means there may be another bounded page to process.
const DEADLINE_PAGE_ROWS: usize = 128;
// Deferred work is revisited within one second; inactive deadlines can sleep for a day.
const DEFERRED_RECHECK_SECONDS: u64 = 1;
const IDLE_DELAY_SECONDS: u64 = 86_400;
// Active deadlines and retry pruning are revisited at least once per minute.
const DEADLINE_RECHECK_SECONDS: u64 = 60;
const RETRY_PRUNE_INTERVAL: Duration = Duration::from_secs(DEADLINE_RECHECK_SECONDS);
const _: () = assert!(DEFERRED_RECHECK_SECONDS <= DEADLINE_RECHECK_SECONDS);
const _: () = assert!(DEADLINE_RECHECK_SECONDS <= IDLE_DELAY_SECONDS);
const _: () = assert!((1_u64 << MAX_RETRY_EXPONENT) >= MAX_RETRY_SECONDS);

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Job {
    Switch(String),
    Restore(String),
}

struct Retry {
    at: std::time::Instant,
    attempt: u32,
}
fn defer(retries: &mut std::collections::BTreeMap<Job, Retry>, key: Job) {
    let attempt = retries
        .get(&key)
        .map_or(0, |r| r.attempt.saturating_add(1).min(MAX_RETRY_EXPONENT));
    retries.insert(
        key,
        Retry {
            at: std::time::Instant::now()
                + Duration::from_secs((1u64 << attempt).min(MAX_RETRY_SECONDS)),
            attempt,
        },
    );
}

pub(in crate::runtime) async fn run(
    shared: Shared,
    pool: Pool,
    shutdown: Arc<AtomicBool>,
) -> Result<()> {
    let mut owner = DeadlineOwner::start(shared, pool, shutdown).await?;
    let result = owner.serve().await;
    owner.drain(result).await
}

/// Ephemeral JSON views of configured groups for deadline SQL.
struct GroupViews {
    /// Group name to member and paid-tail profiles.
    members: String,
    /// Names of balanced groups, whose idle agents keep their binding.
    balanced: String,
}
#[cfg(test)]
impl GroupViews {
    fn empty() -> Self {
        Self {
            members: "{}".into(),
            balanced: "[]".into(),
        }
    }
}

type FinishedJob = std::result::Result<(Job, Result<()>), tokio::task::JoinError>;
// Retain the shared owners until every pending job has been drained or dropped.
struct DeadlineOwner {
    prune_cursor: Option<Job>,
    cursor: String,
    groups: Arc<GroupViews>,
    restore: std::collections::VecDeque<String>,
    retries: std::collections::BTreeMap<Job, Retry>,
    active: HashSet<Job>,
    jobs: tokio::task::JoinSet<(Job, Result<()>)>,
    shutdown: Arc<AtomicBool>,
    pool: Pool,
    shared: Shared,
}
impl DeadlineOwner {
    #[expect(
        clippy::expect_used,
        reason = "Auth groups serialize as string-keyed maps and lists of profile names."
    )]
    async fn start(shared: Shared, pool: Pool, shutdown: Arc<AtomicBool>) -> Result<Self> {
        let jobs = tokio::task::JoinSet::new();
        let active = HashSet::new();
        let retries = std::collections::BTreeMap::<Job, Retry>::new();
        let restore = shared
            .with(|core| core.store().restoration_profiles())
            .await?
            .into();
        let groups = Arc::new(
            shared
                .with(|core| {
                    let config = core.config();
                    let balanced: Vec<_> = config
                        .auth_groups
                        .iter()
                        .filter(|(_, group)| group.balanced())
                        .map(|(name, _)| name)
                        .collect();
                    Ok(GroupViews {
                        members: serde_json::to_string(&config.auth_group_members())
                            .expect("group configuration"),
                        balanced: serde_json::to_string(&balanced).expect("group names"),
                    })
                })
                .await?,
        );
        Ok(Self {
            shared,
            pool,
            shutdown,
            jobs,
            active,
            retries,
            restore,
            groups,
            cursor: String::new(),
            prune_cursor: None,
        })
    }
    // Observe shutdown results without creating more work or submitting writer jobs.
    async fn drain(&mut self, mut result: Result<()>) -> Result<()> {
        if let Err(error) = &result {
            self.shared.wake.failed(error);
            self.shutdown.store(true, Ordering::Release);
            tracing::error!(name: "auth.owner_failed",
                 code = error.code(),
                 error = error as &(dyn std::error::Error + 'static),
                 decision = "drain_owners");
            self.shared.wake.all();
        }
        while let Some(done) = self.jobs.join_next().await {
            match done {
                Ok((key, Err(error))) => record_job_failure(&key, &error, "shutdown_observe"),
                Err(cause) => {
                    let error =
                        Error::internal_error("Login switch owner failed.").with_source(cause);
                    self.shared.wake.failed(&error);
                    tracing::error!(name: "auth.job_join_failed",
                         code = error.code(),
                         error = &error as &(dyn std::error::Error + 'static),
                         decision = "drain_owners");
                    if result.is_ok() {
                        result = Err(error);
                    }
                }
                Ok((_, Ok(()))) => {}
            }
        }
        result
    }
    async fn serve(&mut self) -> Result<()> {
        // Polling and job mutation remain sequential so capacity, cursor and shutdown agree.
        while !self.shutdown.load(Ordering::Acquire) {
            prune_retries(&self.shared, &mut self.retries, &mut self.prune_cursor).await?;
            let now = std::time::Instant::now();
            self.start_restores(now);
            let after = self.cursor.clone();
            let groups = self.groups.clone();
            let DueWork {
                ids,
                deadline,
                more,
            } = self
                .shared
                .with(move |core| core.atomic(|core| due_work(core, &after, &groups)))
                .await?;
            let deferred = self.start_switches(ids, now);
            let delay = self.delay(deadline, now, deferred, more);
            tokio::select! {
                result = self.jobs.join_next(), if !self.jobs.is_empty() => {
                    self.complete(result).await?;
                },
                () = self.shared.wake.deadlines.notified() => {},
                () = tokio::time::sleep(delay) => {},
            }
        }
        Ok(())
    }
    #[expect(
        clippy::expect_used,
        reason = "The initial queue length bounds the loop's one front pop per iteration."
    )]
    fn start_restores(&mut self, now: std::time::Instant) {
        for _ in 0..self.restore.len() {
            if self.jobs.len() >= JOB_LIMIT {
                break;
            }
            let auth = self.restore.pop_front().expect("restoration queue");
            let key = Job::Restore(auth.clone());
            if self.retries.get(&key).is_some_and(|retry| retry.at > now) {
                self.restore.push_back(auth);
                continue;
            }
            let hub = self.shared.clone();
            self.active.insert(key.clone());
            self.jobs.spawn(async move {
                let result = Box::pin(restore_profile(&hub, auth)).await;
                (key, result)
            });
        }
    }
    fn start_switches(&mut self, ids: Vec<String>, now: std::time::Instant) -> bool {
        if ids.is_empty() {
            self.cursor.clear();
        }
        let mut deferred = false;
        for id in ids {
            if self.jobs.len() >= JOB_LIMIT {
                deferred = true;
                break;
            }
            self.cursor.clone_from(&id);
            let key = Job::Switch(id.clone());
            if self.retries.get(&key).is_some_and(|retry| retry.at > now) {
                continue;
            }
            if self.active.insert(key.clone()) {
                let hub = self.shared.clone();
                let pool = self.pool.clone();
                self.jobs
                    .spawn(async move { (key, finish_switch(&hub, &pool, &id).await) });
            }
        }
        deferred
    }
    fn delay(
        &self,
        deadline: Option<u64>,
        now: std::time::Instant,
        deferred: bool,
        more: bool,
    ) -> Duration {
        let seconds = deadline_delay(deadline, crate::state::now());
        let delay = Duration::from_secs(if deferred || more {
            seconds.min(DEFERRED_RECHECK_SECONDS)
        } else {
            seconds
        });
        let delay = self
            .retries
            .values()
            .filter(|r| r.at > now)
            .fold(delay, |delay, retry| {
                delay.min(retry.at.saturating_duration_since(now))
            });
        // Bounded lifecycle pruning continues even after the final intent ends.
        if self.retries.is_empty() {
            delay
        } else {
            delay.min(RETRY_PRUNE_INTERVAL)
        }
    }
    async fn complete(&mut self, result: Option<FinishedJob>) -> Result<()> {
        match result {
            Some(Ok((key, result))) => {
                self.active.remove(&key);
                if let Err(error) = result {
                    if matches!(key, Job::Restore(_))
                        && crate::runtime::provider::catalogs::capacity_exhausted(&error)
                    {
                        self.pool.drain_catalog();
                    }
                    match &key {
                        Job::Switch(id) => {
                            record_job_failure(&key, &error, "record_deferred");
                            if record_deferred(&self.shared, id, &error).await? {
                                defer(&mut self.retries, key);
                            } else {
                                self.retries.remove(&key);
                            }
                        }
                        Job::Restore(auth) if error.is_retryable_restore() => {
                            record_job_failure(&key, &error, "retry");
                            self.restore.push_back(auth.clone());
                            defer(&mut self.retries, key);
                        }
                        Job::Restore(_) => {
                            record_job_failure(&key, &error, "terminal");
                            self.retries.remove(&key);
                        }
                    }
                } else {
                    self.retries.remove(&key);
                }
            }
            Some(Err(cause)) => {
                return Err(Error::internal_error("Login switch owner failed.").with_source(cause));
            }
            None => {}
        }
        Ok(())
    }
}

fn record_job_failure(key: &Job, error: &Error, decision: &'static str) {
    match key {
        Job::Switch(id) => {
            tracing::warn!(name: "auth.switch_failed",
                 agent_id = id.as_str(),
                 code = error.code(),
                 error = error as &(dyn std::error::Error + 'static),
                 decision = decision);
        }
        Job::Restore(_) => {
            tracing::warn!(name: "auth.restore_failed",
                 code = error.code(),
                 error = error as &(dyn std::error::Error + 'static),
                 decision = decision);
        }
    }
}

struct DueWork {
    ids: Vec<String>,
    deadline: Option<u64>,
    more: bool,
}
fn due_work(core: &mut Core, after: &str, groups: &GroupViews) -> Result<DueWork> {
    let now = crate::state::now();
    let joins = core.store().expire_join_deadlines(now)?;
    let branches = core.store().settle_branches(now)?;
    let promoted = core.promote_scheduled(now)?;
    let expired = core.store().expire_login_windows(now)?;
    let probes = core.reconcile_auth_probes(crate::state::now())?;
    let affected = core.store().affected_auth_agents(&groups.balanced)?;
    let waiters = core.store().quota_waiters(&groups.members)?;
    let more = joins == DEADLINE_PAGE_ROWS
        || branches == DEADLINE_PAGE_ROWS
        || promoted == DEADLINE_PAGE_ROWS
        || expired == DEADLINE_PAGE_ROWS
        || probes == DEADLINE_PAGE_ROWS
        || affected.len() == DEADLINE_PAGE_ROWS
        || waiters.len() == DEADLINE_PAGE_ROWS;
    // Each transition can alter the next lookup; preserve writer mutation and event order.
    for id in affected.into_iter().chain(waiters) {
        let mut agent = core.store().agent(&id)?;
        if agent.waiting_reason == Some(WaitingReason::Quota)
            && core.unpark_auth_waiter(&mut agent)?
        {
            continue;
        }
        if agent.branch_unbound() || agent.branch_hold > 0 {
            continue;
        }
        let (target, reason) = agent.auth_switch_pending.as_ref().map_or_else(
            || {
                (
                    agent
                        .auth_group
                        .clone()
                        .unwrap_or_else(|| agent.auth.clone()),
                    AuthReason::UsageLimit,
                )
            },
            |intent| (intent.target.clone(), intent.reason),
        );
        core.request_auth_boundary(&mut agent, target, reason)?;
    }
    let mut ids = core.store().auth_switches_after(after)?;
    if ids.is_empty() && !after.is_empty() {
        ids = core.store().auth_switches_after("")?;
    }
    Ok(DueWork {
        ids,
        deadline: core.store().next_deadline()?,
        more,
    })
}

async fn prune_retries(
    shared: &Shared,
    retries: &mut std::collections::BTreeMap<Job, Retry>,
    after: &mut Option<Job>,
) -> Result<()> {
    use std::ops::Bound::{Excluded, Unbounded};
    let keys: Vec<_> = retries
        .range((after.as_ref().map_or(Unbounded, Excluded), Unbounded))
        .take(DEADLINE_PAGE_ROWS)
        .map(|(key, _)| key.clone())
        .collect();
    if keys.is_empty() {
        *after = None;
        return Ok(());
    }
    *after = keys.last().cloned();
    let stale = shared
        .with(move |core| {
            keys.into_iter()
                .map(|key| stale_retry(core, key))
                .filter_map(Result::transpose)
                .collect::<Result<Vec<_>>>()
        })
        .await?;
    for key in stale {
        retries.remove(&key);
    }
    Ok(())
}

fn stale_retry(core: &Core, key: Job) -> Result<Option<Job>> {
    let Job::Switch(id) = &key else {
        return Ok(None);
    };
    let keep = match core.store().agent(id) {
        Ok(agent) => !agent.terminal() && agent.auth_switch_pending.is_some(),
        Err(Error::AgentPurged { .. } | Error::UnknownAgent { .. }) => false,
        Err(error) => return Err(error),
    };
    Ok((!keep).then_some(key))
}

async fn record_deferred(shared: &Shared, id: &str, error: &Error) -> Result<bool> {
    let (id, kind, code) = (id.to_owned(), error.kind(), error.code().to_owned());
    shared
        .with(move |core| {
            let mut agent = match core.store().agent(&id) {
                Ok(agent) => agent,
                Err(Error::UnknownAgent { .. } | Error::AgentPurged { .. }) => return Ok(false),
                Err(error) => return Err(error),
            };
            if !agent.terminal()
                && agent.auth_switch_pending.is_some()
                && agent.error.as_ref() != Some(&code)
            {
                agent.error = Some(code.clone());
                if kind == crate::error::ErrorCode::ProcessVariantLimit {
                    agent.state = crate::runtime::domain::AgentState::Failed;
                    agent.paused = true;
                    agent.waiting_reason = None;
                    agent.quota_wait_until = None;
                    agent.auth_switch_pending = None;
                }
                core.store().save(&agent)?;
                core.store().event(
                    &agent,
                    EventKind::AuthSwitchDeferred,
                    api::value(events::AuthSwitchDeferred { error: &code }),
                )?;
            }
            Ok(!agent.terminal() && agent.auth_switch_pending.is_some())
        })
        .await
}

// Monotonic clocks can pause during laptop sleep; regularly reconsider wall time.
fn deadline_delay(deadline: Option<u64>, now: u64) -> u64 {
    deadline.map_or(IDLE_DELAY_SECONDS, |at| {
        at.saturating_sub(now).min(DEADLINE_RECHECK_SECONDS)
    })
}

#[cfg(test)]
mod failure_tests;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn automatic_switch_variant_limit_keeps_binding_and_stops_until_resume() {
        let fixture = crate::runtime::test_support::RuntimeFixture::new();
        let error = fixture.hub.with(|core| {
            let mut a = core.store().agent("agent")?;
            a.state = AgentState::Stopped;
            a.turn_id = None;
            a.conversation_clean = true;
            a.agent_options = a.agent_options.changed(a.profile.provider, &[("codex.request_compression".into(), json!(true))].into(), None)?;
            core.config_mut().runtime.max_process_variants = 1;
            core.config_mut().auth_profiles.insert("second".into(), a.profile.clone());
            core.config_mut().auth_groups.insert("plans".into(), crate::config::AuthGroup::ordered(vec!["main".into(), "second".into()]));
            let destination = core.state().register("second", &a.profile)?;
            let mut peer = a.clone(); peer.id = "peer".into(); peer.auth = "second".into(); peer.auth_generation = destination.id;
            peer.agent_options = peer.agent_options.changed(peer.profile.provider, &[("codex.request_compression".into(), json!(false))].into(), None)?;
            core.store().save(&peer)?;
            a.auth_group = Some("plans".into());
            core.auth_signal("main", a.auth_generation, "provider", super::super::signals::Signal::limited(AuthReason::UsageLimit))?;
            core.request_auth_boundary(&mut a, "plans".into(), AuthReason::UsageLimit)?;
            let mut prepared = prepare::Input::capture(core, "plans".into()).after_cleanup(a.clone()).resolve()?;
            for member in &mut prepared.members {
                member.catalog = Some(Ok(vec![crate::runtime::provider::protocol::Model::parse(&json!({"model":a.model,"supportedReasoningEfforts":[{"reasoningEffort":a.effort}]}))]));
            }
            let error = core.atomic(|core| apply_switch(core, &a, &prepared)).unwrap_err();
            assert_eq!(error.code(), "process_variant_limit");
            let after = core.store().agent(&a.id)?;
            assert_eq!(after.auth, a.auth);
            assert_eq!(after.auth_generation, a.auth_generation);
            assert_eq!(after.options_revision, a.options_revision);
            assert_eq!(after.agent_options, a.agent_options);
            Ok(error)
        }).await.unwrap();
        assert!(
            !record_deferred(&fixture.hub, "agent", &error)
                .await
                .unwrap()
        );
        fixture
            .hub
            .with(|core| {
                let a = core.store().agent("agent")?;
                assert_eq!(a.state, AgentState::Failed);
                assert_eq!(a.error.as_deref(), Some("process_variant_limit"));
                assert!(a.paused && a.auth_switch_pending.is_none() && a.waiting_reason.is_none());
                Ok(())
            })
            .await
            .unwrap();
        fixture.finish();
    }
    #[tokio::test]
    async fn retry_pruning_drops_cancelled_switches_and_retains_pending_backoff() {
        let fixture = crate::runtime::test_support::RuntimeFixture::new();
        fixture
            .hub
            .with(|core| {
                let mut a = core.store().agent("agent")?;
                core.request_auth_boundary(&mut a, "main".into(), AuthReason::Manual)?;
                Ok(())
            })
            .await
            .unwrap();
        let mut retries = std::collections::BTreeMap::new();
        let pending = Job::Switch("agent".into());
        let absent = Job::Switch("absent".into());
        defer(&mut retries, pending.clone());
        defer(&mut retries, pending.clone());
        defer(&mut retries, absent.clone());
        let original = retries[&pending].at;
        let mut cursor = None;
        prune_retries(&fixture.hub, &mut retries, &mut cursor)
            .await
            .unwrap();
        assert!(!retries.contains_key(&absent));
        assert_eq!(retries[&pending].attempt, 1);
        assert_eq!(retries[&pending].at, original);
        fixture
            .hub
            .with(|core| {
                let mut a = core.store().agent("agent")?;
                a.auth_switch_pending = None;
                core.store().save(&a)
            })
            .await
            .unwrap();
        cursor = None;
        prune_retries(&fixture.hub, &mut retries, &mut cursor)
            .await
            .unwrap();
        assert!(retries.is_empty());
        fixture.finish();
    }
    #[tokio::test]
    async fn purged_switch_completion_is_settled_instead_of_failing_deadline_owner() {
        let fixture = crate::runtime::test_support::RuntimeFixture::new();
        fixture
            .hub
            .with(|core| {
                let mut a = core.store().agent("agent")?;
                a.state = AgentState::Stopped;
                a.task_outcome = crate::runtime::domain::TaskOutcome::Cancelled;
                a.turn_id = None;
                a.conversation_clean = true;
                a.ended_at = Some(crate::state::now());
                core.store().undeliverable(&a.id)?;
                core.store().save(&a)?;
                core.store().purge(&a.id, false, false, core.running())?;
                Ok(())
            })
            .await
            .unwrap();
        let error = fixture
            .hub
            .with(|core| core.store().agent("agent"))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "agent_purged");
        assert!(
            !record_deferred(&fixture.hub, "agent", &error)
                .await
                .unwrap()
        );
        fixture.finish();
    }
    #[tokio::test]
    async fn corrupt_scheduled_row_does_not_block_auth_or_valid_deadlines() {
        let fixture = crate::runtime::test_support::RuntimeFixture::new();
        fixture
            .hub
            .with(|core| {
                core.atomic(|core| {
                    let mut a = core.store().agent("agent")?;
                    a.state = AgentState::Stopped;
                    a.turn_id = None;
                    a.conversation_clean = true;
                    core.store().save(&a)?;
                    core.store()
                        .enqueue_scheduled("bad", "human", "agent", "bad", 1)?; // Deliberately missing call proof.
                    core.store()
                        .enqueue_scheduled("good", "human", "agent", "good", 1)?;
                    core.store().track_call("good")?;
                    let mut availability =
                        super::super::Availability::available("main", a.auth_generation);
                    availability.state = super::super::AvailabilityState::Limited;
                    availability.until = Some(2);
                    core.store().save_availability(&availability)?;
                    assert_eq!(core.store().next_deadline()?, Some(1));
                    let DueWork { deadline, more, .. } = due_work(core, "", &GroupViews::empty())?;
                    assert_eq!(deadline, None);
                    assert!(!more);
                    assert_eq!(
                        core.store().availability("main", a.auth_generation)?.state,
                        super::super::AvailabilityState::Available
                    );
                    assert_eq!(
                        core.store().message_result("good")?.delivery_state,
                        crate::runtime::domain::MessageState::Accepted
                    );
                    assert_eq!(
                        core.store().message_result("bad").unwrap_err().code(),
                        "database_error"
                    );
                    assert_eq!(due_work(core, "", &GroupViews::empty())?.deadline, None);
                    Ok(())
                })
            })
            .await
            .unwrap();
        fixture.finish();
    }

    #[tokio::test]
    async fn automatic_boundary_retains_invalid_probe_and_selects_healthy_alternative() {
        let fixture = crate::runtime::test_support::RuntimeFixture::new();
        fixture.hub.with(|core| {
            let mut a=core.store().agent("agent")?;
            a.state=AgentState::Stopped;a.turn_id=None;a.conversation_clean=true;
            a.auth_group=Some("plans".into());a.auth_probe_id=Some("bad-lease".into());
            core.config_mut().auth_profiles.insert("main".into(),a.profile.clone());
            core.config_mut().auth_profiles.insert("second".into(),a.profile.clone());
            core.config_mut().auth_groups.insert("plans".into(),crate::config::AuthGroup::ordered(vec!["main".into(),"second".into()]));
            core.state().register("second",&a.profile)?;
            let mut value=super::super::Availability::available("main",a.auth_generation);
            value.retry_at=Some(1);value.reason=Some(AuthReason::UsageLimit);value.probe = Some(crate::runtime::auth_fallback::Probe { id: a.auth_probe_id.clone().expect("probe identity"), agent: a.id.clone(), message: "missing".into(), deadline: 1, error: Some(crate::runtime::auth_fallback::ProbeError::MissingInput) });core.store().save_availability(&value)?;
            core.request_auth_boundary(&mut a,"plans".into(),AuthReason::UsageLimit)?;
            let mut prepared=prepare::Input::capture(core,"plans".into()).after_cleanup(a.clone()).resolve()?;
            for member in &mut prepared.members {
                member.catalog=Some(Ok(vec![crate::runtime::provider::protocol::Model::parse(&json!({"model":a.model,"supportedReasoningEfforts":[{"reasoningEffort":a.effort}]}))]));
            }
            apply_switch(core,&a,&prepared)?;
            let moved=core.store().agent(&a.id)?;
            assert_eq!(moved.auth,"second");assert!(moved.auth_probe_id.is_none());
            let retained=core.store().availability("main",a.auth_generation)?;
            assert_eq!(retained.probe_error(),value.probe_error());assert_eq!(retained.probe_id(),value.probe_id());
            Ok(())
        }).await.unwrap();
        fixture.finish();
    }
}

#[cfg(test)]
mod deadline_tests {
    #[test]
    fn sleep_rechecks_wall_deadlines_within_sixty_awake_seconds() {
        assert_eq!(super::deadline_delay(Some(99999), 10), 60);
        assert_eq!(super::deadline_delay(Some(20), 10), 10);
        assert_eq!(super::deadline_delay(Some(20), 30), 0);
        assert_eq!(super::deadline_delay(None, 30), 86400);
    }
}
