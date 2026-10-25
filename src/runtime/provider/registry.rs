//! Provider reservations, capacity accounting, quarantine and spare retention.
use crate::state::RecoveryReservation;
use std::time::Instant;
mod capacity;
use super::{acquire::Acquisition, pool::Pool, shard::Shard};
use crate::runtime::locks::MutexExt;
pub(super) use capacity::{CapacityPressure, Eviction, EvictionPolicy, begin_capacity_drain};
use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};
static ABSENCE_FAILURES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static SPARE_FAILURES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn confirmed_absent(process: &crate::state::ProcessIdentity, provider: &str) -> bool {
    match process.absent() {
        Ok(gone) => gone,
        Err(error) => {
            if let Some(count) = crate::observability::sample(&ABSENCE_FAILURES) {
                tracing::warn!(name: "provider.absence_check_failed",
                     provider_id = provider,
                     pid = process.pid,
                     count = count,
                     error = &error as &(dyn std::error::Error + 'static),
                     decision = "retain_capacity");
            }
            false
        }
    }
}

pub(crate) struct RecoveredReservation {
    pub(crate) reservation: crate::state::RecoveryReservation,
    pub(crate) slots: usize,
    /// Missing after daemon recovery: never infer a reusable identity.
    pub(crate) process_options: Option<String>,
}

#[derive(Default, Debug, PartialEq, Eq)]
pub(super) struct RecoveryCapacity {
    pub(super) processes: usize,
    pub(super) slots: usize,
}

pub(crate) struct Registry {
    pub(super) probes: usize,
    pub(super) shards: Vec<Arc<Shard>>,
    pub(super) recovered: Vec<RecoveredReservation>,
    pub(super) unresolved_auth: std::collections::HashSet<String>,
}
impl Registry {
    pub(super) fn refresh(&mut self) {
        // Process-absence checks authorize capacity release and retirement observations.
        for shard in &self.shards {
            if shard.exited()
                && shard
                    .process
                    .lock_or_panic()
                    .as_ref()
                    .is_some_and(|process| confirmed_absent(process, &shard.id))
            {
                shard.release_capacity();
            }
        }
        self.shards
            .retain(|shard| !(shard.exited() && shard.capacity_released()));
        for RecoveredReservation { reservation, .. } in &mut self.recovered {
            if reservation.unresolved_activity {
                self.unresolved_auth.insert(reservation.auth.clone());
            }
            if !reservation.gone
                && reservation
                    .process
                    .as_ref()
                    .is_some_and(|process| confirmed_absent(process, &reservation.provider))
            {
                reservation.gone = true;
            }
        }
        self.recovered.retain(|r| {
            !r.reservation.gone
                && !r.reservation.unknown_capacity
                && r.reservation.process.is_some()
        });
    }
    pub(super) fn recovered_capacity(&self) -> RecoveryCapacity {
        self.recovered
            .iter()
            .filter(|RecoveredReservation { reservation: r, .. }| {
                !r.gone && !r.unknown_capacity && r.process.is_some()
            })
            .fold(RecoveryCapacity::default(), |mut capacity, recovered| {
                capacity.processes += 1;
                capacity.slots += recovered.slots;
                capacity
            })
    }
}
impl Pool {
    /// Last-user release and acquisition must make one fenced ownership decision.
    /// A lingering release uses the ordinary clean session-release reply; it never
    /// carries a stale last-user close into a newly attached conversation.
    pub(super) fn release_user(&self, shard: &Shard, agent: &str, clean_retirement: bool) -> bool {
        let _registry = self.registry.lock_or_panic();
        let mut occupancy = shard.occupancy();
        occupancy.users.remove(agent);
        occupancy.activity.remove(agent);
        if !occupancy.users.is_empty() {
            return false;
        }
        if clean_retirement
            && shard.backend.multiplexed()
            && self.settings.sessions_per_codex_process > 1
            && self.settings.linger_seconds != 0
            && occupancy.slots.len() < self.settings.sessions_per_codex_process
            && !shard.draining()
            && !shard.stop()
            && !self.shutdown.load(Ordering::Relaxed)
        {
            occupancy.idle_since = Some(Instant::now());
            shard.require_empty_attach_check();
            self.changed.notify_waiters();
            false
        } else {
            shard.begin_drain();
            true
        }
    }

    pub(super) fn retire_record(&self, shard: &Shard) {
        let mut registry = self.registry.lock_or_panic();
        if !shard.capacity_released() {
            registry.recovered.push(RecoveredReservation {
                reservation: RecoveryReservation {
                    auth: shard.auth.clone(),
                    provider: shard.id.clone(),
                    process: shard.process.lock_or_panic().clone(),
                    gone: false,
                    unknown_capacity: false,
                    unresolved_activity: shard
                        .report
                        .lock_or_panic()
                        .as_ref()
                        .is_none_or(|report| !report.quiescent),
                },
                slots: if shard.backend.multiplexed() {
                    self.settings.sessions_per_codex_process
                } else {
                    1
                },
                process_options: Some(shard.process_options.clone()),
            });
        }
        registry.shards.retain(|s| s.id != shard.id);
        self.changed.notify_waiters();
        self.wake.all();
    }

    pub(super) fn quarantine(&self, shard: &Shard) {
        shard.begin_drain();
        shard.request_stop();
        shard.wake_owner();
        self.wake.all();
        self.changed.notify_waiters();
    }
    pub(super) fn drain_one(&self, registry: &Registry, pressure: CapacityPressure) {
        if let Eviction::Victim(shard) = registry.eviction_victim(
            EvictionPolicy::Runtime(pressure),
            Duration::from_secs(self.settings.cohort_min_seconds),
        ) {
            begin_capacity_drain(shard, EvictionPolicy::Runtime(pressure), Some(&self.wake));
        }
    }
}
impl Acquisition<'_> {
    pub(super) fn start_spares(&self, registry: &mut Registry) {
        if !self.command.backend.multiplexed() {
            return;
        }
        // Spares are best effort, only from genuine headroom after demand.
        // They have no threads or agent identities and expire without replenishment.
        while self.pool.settings.warm_providers
            > registry
                .shards
                .iter()
                .filter(|s| {
                    let occupancy = s.occupancy();
                    self.same_executable(s)
                        && s.auth == self.auth
                        && s.process_options == self.command.process_options
                        && s.key.as_ref() == self.command.credential.authority()
                        && s.lease.generation() == self.lease.generation()
                        && occupancy.users.is_empty()
                        && (occupancy.slots.is_empty() || occupancy.idle_since.is_some())
                        && occupancy.slots.len() < self.pool.settings.sessions_per_codex_process
                        && !s.draining()
                        && !s.stop()
                })
                .count()
            && registry.room(&self.pool.settings).can_start()
            && self.variant_room(registry)
        {
            if self
                .pool
                .start_shard(
                    registry,
                    self.auth,
                    self.command,
                    self.lease.clone(),
                    self.daemon_lock.clone(),
                )
                .inspect_err(|error| {
                    if let Some(count) = crate::observability::sample(&SPARE_FAILURES) {
                        tracing::warn!(name: "provider.spare_start_failed",
                             count = count,
                             error = error as &(dyn std::error::Error + 'static),
                             decision = "retain_demand_capacity");
                    }
                })
                .is_err()
            {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_pressure_skips_empty_linger_but_process_pressure_reclaims_it() {
        use crate::runtime::{provider::shard::corpus_support, test_support::RuntimeFixture};
        let fixture = RuntimeFixture::new();
        let pool = fixture.pool();
        let corpus_support::ShardSetup {
            shard: empty,
            queues: _queues,
        } = corpus_support::shard(&fixture);
        empty.occupancy().slots.insert("retired".into());
        empty.occupancy().idle_since = Some(std::time::Instant::now());
        {
            let mut registry = pool.registry.lock_or_panic();
            registry.shards.push(empty.clone());
            pool.drain_one(&registry, CapacityPressure::Actors);
            assert!(!empty.draining());
            pool.drain_one(&registry, CapacityPressure::Processes);
            assert!(empty.draining());
        }
        drop(pool);
        drop(empty);
        fixture.finish();
    }
    #[tokio::test]
    async fn retiring_quiescent_work_does_not_add_unresolved_auth_history() {
        use crate::runtime::{
            provider::{rpc::Shutdown, shard::corpus_support::WorkerTransport},
            test_support::RuntimeFixture,
        };
        let fixture = RuntimeFixture::new();
        for quiescent in [true, false] {
            let transport = WorkerTransport::new(&fixture, serde_json::json!({}));
            *transport.session.shard.report.lock_or_panic() = Some(Shutdown {
                method: crate::runtime::provider::ShutdownMethod::Eof,
                confirmed: false,
                quiescent,
                reaped: false,
                exit_code: None,
            });
            transport
                .session
                .pool
                .retire_record(&transport.session.shard);
            {
                let mut registry = transport.session.pool.registry.lock_or_panic();
                registry.refresh();
                assert_eq!(registry.unresolved_auth.contains("main"), !quiescent);
            }
            transport.finish().await;
        }
        fixture.finish();
    }
    #[tokio::test]
    async fn an_old_spare_gets_its_cohort_minimum_from_session_readiness() {
        use crate::runtime::{
            provider::shard::corpus_support::WorkerTransport, test_support::RuntimeFixture,
        };
        let fixture = RuntimeFixture::new();
        let mut transport = WorkerTransport::new(&fixture, serde_json::json!({}));
        Arc::get_mut(&mut transport.session.shard).unwrap().created = std::time::Instant::now()
            .checked_sub(Duration::from_secs(7200))
            .unwrap();
        transport.session.pool.settings.cohort_min_seconds = 3600;
        transport
            .session
            .shard
            .occupancy()
            .slots
            .insert("agent".into());
        transport.session.mark_ready();
        transport
            .session
            .shard
            .occupancy()
            .users
            .insert("agent".into());
        {
            let mut registry = transport.session.pool.registry.lock_or_panic();
            registry.shards.push(transport.session.shard.clone());
            transport
                .session
                .pool
                .drain_one(&registry, CapacityPressure::Processes);
            assert!(!transport.session.draining());
        }
        transport.session.pool.settings.cohort_min_seconds = 0;
        {
            let mut registry = transport.session.pool.registry.lock_or_panic();
            transport
                .session
                .pool
                .drain_one(&registry, CapacityPressure::Processes);
            assert!(transport.session.draining());
            registry.shards.clear();
        }
        transport.finish().await;
        fixture.finish();
    }
    #[test]
    fn unresolved_recovered_processes_keep_capacity_until_exact_absence() {
        let current = crate::state::ProcessIdentity::current().unwrap();
        let mut registry = Registry {
            probes: 0,
            unresolved_auth: std::collections::HashSet::new(),
            shards: vec![],
            recovered: vec![RecoveredReservation {
                process_options: None,
                reservation: crate::state::RecoveryReservation {
                    auth: "main".into(),
                    provider: "provider".into(),
                    process: Some(current),
                    gone: false,
                    unknown_capacity: false,
                    unresolved_activity: true,
                },
                slots: 3,
            }],
        };
        registry.refresh();
        assert_eq!(
            registry.recovered_capacity(),
            RecoveryCapacity {
                processes: 1,
                slots: 3
            }
        );
        registry.recovered[0]
            .reservation
            .process
            .as_mut()
            .unwrap()
            .birth = if cfg!(target_os = "macos") {
            "macos:0:0".into()
        } else {
            "linux:0".into()
        };
        registry.refresh();
        assert_eq!(
            registry.recovered_capacity(),
            RecoveryCapacity {
                processes: 0,
                slots: 0
            }
        );
        assert!(registry.recovered.is_empty());
        assert!(registry.unresolved_auth.contains("main"));
        registry.refresh();
        assert!(registry.unresolved_auth.contains("main"));
        assert_eq!(
            registry.recovered_capacity(),
            RecoveryCapacity {
                processes: 0,
                slots: 0
            }
        );
    }
}
