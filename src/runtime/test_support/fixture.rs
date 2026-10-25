//! Runtime fixtures retain preflight, writer, lease, launcher and daemon ownership through cleanup.
use super::{CoreBuilder, agent};
use crate::runtime::{
    hub::{Hub, StartedHub, Wake},
    provider::Pool,
    store::Store,
};
use crate::{
    config::Runtime,
    state::{Lock, RuntimeLease, State},
};
use serde_json::Value;
use std::sync::{Arc, atomic::AtomicBool};

pub(crate) struct RuntimeOwners {
    pub temp: tempfile::TempDir,
    pub hub: Hub,
    pub writer: std::thread::JoinHandle<crate::error::Result<()>>,
    pub lease: Arc<RuntimeLease>,
    pub launcher: Arc<crate::launch::LauncherCache>,
    pub daemon_lock: Arc<Lock>,
    preflight: crate::runtime::preflight::Owner,
}

pub(crate) struct RuntimeFixture(Option<RuntimeOwners>);
impl std::ops::Deref for RuntimeFixture {
    type Target = RuntimeOwners;
    fn deref(&self) -> &RuntimeOwners {
        self.0.as_ref().unwrap()
    }
}
impl RuntimeFixture {
    pub fn new() -> Self {
        let temp = crate::runtime::test_directory();
        let state = State::new(temp.path().join("state")).unwrap();
        let mut agent = agent();
        // Keep another test's fork from retaining registration's exclusive OFD
        // until exec while this fixture immediately acquires its runtime lease.
        let fork = crate::os::fork_guard().unwrap();
        let binding = state.register("main", &agent.profile).unwrap();
        agent.auth_generation = binding.id;
        crate::state::directory(&state.root.join("runtime")).unwrap();
        let daemon = Arc::new(Lock::acquire(&state.root.join("runtime/daemon.lock")).unwrap());
        let lease = Arc::new(state.runtime_lease(&binding).unwrap());
        drop(fork);
        let store = Store::open(&temp.path().join("runtime.sqlite")).unwrap();
        store.save(&agent).unwrap();
        store
            .enqueue("message", "human", "agent", "corpus input")
            .unwrap();
        let launcher = crate::launch::test_cache().unwrap();
        let core =
            CoreBuilder::from_store(temp.path(), store, state, daemon.clone(), launcher.clone())
                .build();
        let StartedHub {
            hub,
            writer,
            preflight,
        } = Hub::start(core, Arc::new(Wake::default()), Arc::default()).unwrap();
        Self(Some(RuntimeOwners {
            temp,
            hub,
            writer,
            lease,
            launcher,
            daemon_lock: daemon,
            preflight,
        }))
    }

    pub fn pool(&self) -> Pool {
        Pool::new(
            Runtime::default(),
            self.hub.clone(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            vec![],
        )
    }

    pub fn events(&self) -> Value {
        self.hub
            .blocking_with(|core| core.store().events("agent", 0, 200))
            .unwrap()
    }

    pub fn finish_failed(mut self) -> crate::error::Error {
        let cleanup = self.0.take().unwrap().close();
        cleanup.preflight.unwrap();
        cleanup.writer.unwrap().unwrap_err()
    }
    pub fn finish(mut self) {
        let cleanup = self.0.take().unwrap().close();
        cleanup.preflight.unwrap();
        cleanup.writer.unwrap().unwrap();
    }
}
struct Cleanup {
    preflight: crate::error::Result<()>,
    writer: std::thread::Result<crate::error::Result<()>>,
}
impl RuntimeOwners {
    fn close(self) -> Cleanup {
        let Self {
            temp,
            hub,
            writer,
            lease,
            launcher,
            daemon_lock,
            preflight,
        } = self;
        let preflight = preflight.finish();
        drop(hub);
        let writer = writer.join();
        drop(daemon_lock);
        drop(lease);
        drop(temp);
        drop(launcher);
        Cleanup { preflight, writer }
    }
}
impl Drop for RuntimeFixture {
    fn drop(&mut self) {
        let Some(owners) = self.0.take() else {
            return;
        };
        if std::thread::panicking() {
            // Tokio tasks may still retain a Hub until their runtime unwinds.
            // Keep every owner alive until the writer exits. If thread creation fails,
            // retain ownership until process exit rather than deleting a live fixture.
            let owners = std::mem::ManuallyDrop::new(owners);
            let _ = std::thread::Builder::new()
                .name("fixture-cleanup".into())
                .spawn(move || {
                    std::mem::ManuallyDrop::into_inner(owners).close();
                });
        } else {
            let cleanup = owners.close();
            cleanup.preflight.unwrap();
            cleanup.writer.unwrap().unwrap();
        }
    }
}

#[tokio::test]
async fn panic_cleanup_waits_for_async_hub_owners_before_removing_the_directory() {
    let fixture = RuntimeFixture::new();
    let root = fixture.temp.path().to_owned();
    let retained = fixture.hub.clone();
    let launcher = Arc::downgrade(&fixture.launcher);
    let task = tokio::spawn(async move {
        let _retained = retained;
        std::future::pending::<()>().await;
    });
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _fixture = fixture;
        panic!("exercise fixture cleanup");
    }));
    assert!(result.is_err());
    assert!(root.exists());
    assert!(launcher.upgrade().is_some());
    crate::test_support::assert_code!(
        Lock::acquire(&root.join("state/runtime/daemon.lock")),
        "profile_busy"
    );
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::task::spawn_blocking(move || {
        crate::test_support::wait_until(
            crate::test_support::TEST_TIMEOUT,
            std::time::Duration::from_millis(1),
            || (!root.exists()).then_some(()),
        );
    })
    .await
    .unwrap();
}
