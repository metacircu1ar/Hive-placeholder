use super::*;
use crate::runtime::{
    daemon::{DaemonSignals, daemon_signals},
    domain::AgentState,
    provider::Pool,
    test_support::{RuntimeFixture, agent},
};
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::{sync::Semaphore, task::JoinSet};

struct Fixture {
    runtime: RuntimeFixture,
    settings: crate::config::Runtime,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
    uncertain: Arc<std::sync::atomic::AtomicBool>,
    pool: Pool,
    signals: DaemonSignals,
    workers: JoinSet<String>,
    occupied: HashSet<String>,
    starting: Arc<Semaphore>,
    turns: Arc<Semaphore>,
    maintenance: tokio::time::Interval,
}
impl Fixture {
    async fn new(count: usize) -> Self {
        let runtime = RuntimeFixture::new();
        let shutdown = runtime
            .hub
            .with(|core| Ok(core.shutdown().clone()))
            .await
            .unwrap();
        let uncertain = Arc::<std::sync::atomic::AtomicBool>::default();
        let settings = crate::config::Runtime {
            max_loaded: 128,
            ..crate::config::Runtime::default()
        };
        let pool = Pool::new(
            settings.clone(),
            runtime.hub.clone(),
            uncertain.clone(),
            shutdown.clone(),
            vec![],
        );
        runtime
            .hub
            .with(move |core| {
                for i in 0..count {
                    let mut a = agent();
                    a.id = format!("worker-{i:03}");
                    a.parent_id = Some("agent".into());
                    a.state = AgentState::Idle;
                    a.thread_id = None;
                    a.execution_id = None;
                    a.turn_id = None;
                    core.store().save(&a)?;
                    core.store().enqueue(&a.id, "human", &a.id, "next turn")?;
                    assert_eq!(core.store().claim(1)?, Some(a.id.clone()));
                    core.started(a.id);
                }
                Ok(())
            })
            .await
            .unwrap();
        let occupied: HashSet<_> = (0..count).map(|i| format!("worker-{i:03}")).collect();
        for id in &occupied {
            runtime.hub.wake.register(id);
        }
        Self {
            runtime,
            settings,
            shutdown,
            uncertain,
            pool,
            signals: daemon_signals().unwrap(),
            workers: JoinSet::new(),
            occupied,
            starting: Arc::new(Semaphore::new(32)),
            turns: Arc::new(Semaphore::new(256)),
            maintenance: tokio::time::interval(Duration::from_secs(60)),
        }
    }
    fn supervisor(&mut self) -> Supervisor<'_> {
        Supervisor {
            settings: &self.settings,
            shutdown: &self.shutdown,
            cleanup_uncertain: &self.uncertain,
            shared: &self.runtime.hub,
            pool: &self.pool,
            wake: &self.runtime.hub.wake,
            sigint: &mut self.signals.sigint,
            sigterm: &mut self.signals.sigterm,
            sighup: &mut self.signals.sighup,
            workers: &mut self.workers,
            occupied: &mut self.occupied,
            starting: &self.starting,
            turns: &self.turns,
            maintenance: &mut self.maintenance,
        }
    }
    async fn returned(&mut self, i: usize) {
        let task = self.workers.spawn(async move { format!("worker-{i:03}") });
        while !task.is_finished() {
            tokio::task::yield_now().await;
        }
    }
    fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.runtime.temp.path().join("runtime.sqlite")).unwrap()
    }
    fn claims(&self) -> i64 {
        self.db()
            .query_row("SELECT count(*) FROM agents WHERE claimed=1", [], |row| {
                row.get(0)
            })
            .unwrap()
    }
    async fn finish(mut self) {
        self.workers.abort_all();
        while self.workers.join_next().await.is_some() {}
        drop(self.pool);
        self.runtime.finish();
    }
}

#[tokio::test]
async fn control_failure_does_not_cancel_an_inflight_claim_release() {
    let mut f = Fixture::new(1).await;
    f.returned(0).await;
    f.runtime.hub.wake.supervisor.notified().await; // Clear the fixture's setup wake.
    let (entered, entering) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let shared = f.runtime.hub.clone();
    let blocked = tokio::spawn(async move {
        shared
            .with(move |_| {
                entered.send(()).unwrap();
                wait.recv().unwrap();
                Ok(())
            })
            .await
    });
    entering.await.unwrap();
    let (started, starting) = tokio::sync::oneshot::channel();
    let mut control = tokio::spawn(async move {
        starting.await.unwrap();
        Err(crate::error::Error::daemon_stopping(
            "control failure fixture",
        ))
    });
    let wake = f.runtime.hub.wake.clone();
    let shutdown = f.shutdown.clone();
    let uncertain = f.uncertain.clone();
    let acknowledged = std::sync::atomic::AtomicBool::new(false);
    let flush = async {
        started.send(()).unwrap();
        let mut budget = BATCH_LIMIT;
        let result = f.supervisor().collect_finished(&mut budget).await;
        acknowledged.store(true, Ordering::Relaxed);
        result
    };
    let unblock = async {
        wake.supervisor.notified().await;
        assert!(shutdown.load(Ordering::Relaxed));
        assert!(!uncertain.load(Ordering::Relaxed));
        release.send(()).unwrap();
    };
    let (outcome, ()) = tokio::join!(
        crate::runtime::daemon::control::supervise(
            flush,
            &mut control,
            &shutdown,
            &uncertain,
            &wake
        ),
        unblock,
    );
    assert!(outcome.supervisor.is_ok());
    assert!(outcome.control.unwrap().is_err());
    assert!(acknowledged.load(Ordering::Relaxed));
    blocked.await.unwrap().unwrap();
    assert_eq!(f.claims(), 0);
    assert!(f.occupied.is_empty());
    f.finish().await;
}

#[tokio::test]
async fn successful_harvest_publishes_a_wake_before_its_commit_acknowledgement() {
    fn ready(wake: &crate::runtime::hub::Wake) -> bool {
        let mut notified = std::pin::pin!(wake.supervisor.notified());
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        std::future::Future::poll(notified.as_mut(), &mut context).is_ready()
    }
    let mut f = Fixture::new(1).await;
    // Consume the setup transaction's wake. No other task publishes one.
    assert!(ready(&f.runtime.hub.wake));
    assert!(!ready(&f.runtime.hub.wake));
    f.returned(0).await;
    let mut budget = BATCH_LIMIT;
    f.supervisor().collect_finished(&mut budget).await.unwrap();
    assert_eq!(budget, BATCH_LIMIT - 1);
    assert_eq!(f.claims(), 0);
    assert!(ready(&f.runtime.hub.wake));
    f.finish().await;
}

#[tokio::test]
async fn bounded_harvest_commits_claims_and_shares_the_iteration_budget() {
    let mut f = Fixture::new(BATCH_LIMIT + 1).await;
    for i in 0..=BATCH_LIMIT {
        f.returned(i).await;
    }
    let mut budget = BATCH_LIMIT;
    f.supervisor().collect_finished(&mut budget).await.unwrap();
    assert_eq!(budget, 0);
    assert_eq!(f.occupied.len(), 1);
    assert_eq!(f.claims(), 1);
    assert_eq!(
        f.runtime.hub.with(|c| Ok(c.running().len())).await.unwrap(),
        1
    );
    f.supervisor().collect_finished(&mut budget).await.unwrap();
    assert_eq!(f.workers.len(), 1);
    // The ordinary event path is allowed one additional completion.
    let last = f.workers.join_next().await;
    f.supervisor().worker_finished(last).await.unwrap();
    assert!(f.occupied.is_empty());
    assert_eq!(f.claims(), 0);
    assert_eq!(
        f.runtime.hub.with(|c| Ok(c.running().len())).await.unwrap(),
        0
    );
    f.finish().await;
}

#[tokio::test]
async fn panic_and_abort_stop_harvesting_but_flush_prior_successes() {
    for abort in [false, true] {
        let mut f = Fixture::new(3).await;
        f.returned(0).await;
        let task = f.workers.spawn(async move {
            if abort {
                std::future::pending::<()>().await;
            }
            panic!("completed worker fixture");
        });
        if abort {
            task.abort();
        }
        while !task.is_finished() {
            tokio::task::yield_now().await;
        }
        f.returned(2).await;
        let mut budget = BATCH_LIMIT;
        f.supervisor().collect_finished(&mut budget).await.unwrap();
        assert!(f.uncertain.load(Ordering::Relaxed));
        assert_eq!(budget, BATCH_LIMIT - 2);
        assert_eq!(f.claims(), 2);
        assert!(!f.occupied.contains("worker-000"));
        assert_eq!(f.workers.len(), 1);
        f.finish().await;
    }
}

#[tokio::test]
async fn batch_release_failure_rolls_back_claims_and_exits_before_admission() {
    let mut f = Fixture::new(3).await;
    for i in 0..3 {
        f.returned(i).await;
    }
    f.db().execute_batch("CREATE TRIGGER fail_claim_release BEFORE UPDATE OF claimed ON agents
        WHEN new.claimed=0 AND old.id='worker-001' BEGIN SELECT RAISE(ABORT,'fixture failure'); END;").unwrap();
    assert!(f.supervisor().run().await.is_err());
    assert_eq!(f.claims(), 3);
    assert!(f.occupied.is_empty());
    assert!(f.workers.is_empty());
    assert!(!f.uncertain.load(Ordering::Relaxed));
    assert!(
        !f.runtime
            .hub
            .with(|c| Ok(c.running().contains("worker-001")))
            .await
            .unwrap()
    );
    f.finish().await;
}

#[tokio::test]
async fn stopping_prevents_new_harvest_but_does_not_skip_collected_claim_release() {
    let mut f = Fixture::new(2).await;
    f.returned(0).await;
    f.returned(1).await;
    let done = f.workers.join_next().await;
    f.shutdown.store(true, Ordering::Relaxed);
    f.supervisor().worker_finished(done).await.unwrap();
    assert_eq!(f.claims(), 1);
    let mut budget = BATCH_LIMIT;
    f.supervisor().collect_finished(&mut budget).await.unwrap();
    assert_eq!(budget, BATCH_LIMIT);
    assert_eq!(f.workers.len(), 1);
    f.finish().await;
}
