//! Admission resources and lifecycle owners for session execution tests.
use super::ActiveTurn;
use crate::error::Result;
use crate::runtime::domain::MessageState;
use crate::runtime::{domain::Agent, provider::WorkerTransport, test_support::RuntimeFixture};
use serde_json::Value;
use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Instant,
};
use tokio::sync::{Notify, Semaphore};

pub(super) async fn execute(
    fixture: &RuntimeFixture,
    agent: Agent,
    transport: &mut WorkerTransport,
) -> Result<bool> {
    let starting = Arc::new(Semaphore::new(1)).acquire_owned().await.unwrap();
    Box::pin(super::execute_session(
        super::ExecutionHandle::new(&fixture.hub, "agent", "execution"),
        super::SessionInputs {
            agent,
            rpc: &mut transport.session,
            shutdown: Arc::new(AtomicBool::new(true)),
            notify: Arc::new(Notify::new()),
            starting,
            turns: Arc::new(Semaphore::new(1)),
            permit: &mut None,
            idle_seconds: 60,
        },
    ))
    .await
}

pub(super) fn execution<'a>(
    fixture: &'a RuntimeFixture,
    agent: &'a Agent,
) -> super::ExecutionHandle<'a> {
    super::ExecutionHandle::new(
        &fixture.hub,
        &agent.id,
        agent.execution_id.as_deref().unwrap(),
    )
}

pub(super) async fn active(fixture: &RuntimeFixture) -> Option<ActiveTurn> {
    let message = fixture
        .hub
        .with(|core| {
            let message = core.store().next_message("agent")?.unwrap();
            core.store()
                .message_state(&message, MessageState::Submitted)?;
            Ok(message)
        })
        .await
        .unwrap();
    Some(ActiveTurn {
        message,
        turn_id: "turn".into(),
        started: Instant::now(),
    })
}

pub(super) async fn events(fixture: &RuntimeFixture) -> Value {
    fixture
        .hub
        .with(|core| core.store().events("agent", 0, 200))
        .await
        .unwrap()
}

pub(super) fn replay_ids(fixture: &RuntimeFixture) -> Vec<String> {
    let connection =
        rusqlite::Connection::open(fixture.temp.path().join("runtime.sqlite")).unwrap();
    connection
        .prepare("SELECT id FROM requests WHERE sender='agent' ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}
