//! Session deadline precedence without provider I/O or wall-clock sleeps.
use super::super::{ActiveTurn, NO_DEADLINE_WAIT, TURN_TIMEOUT, steering};
use super::*;
use crate::runtime::{provider::WorkerTransport, test_support::RuntimeFixture};
use serde_json::json;
use std::sync::{Arc, atomic::AtomicBool};
use tokio::sync::{Notify, Semaphore};

async fn with_session(check: impl FnOnce(&mut SessionLoop<'_>)) {
    let fixture = RuntimeFixture::new();
    let agent = fixture
        .hub
        .with(|core| core.store().agent("agent"))
        .await
        .unwrap();
    let mut transport = WorkerTransport::new(&fixture, json!({}));
    {
        let mut driver = SessionLoop {
            applied_turn_selection: agent.desired_selection(),
            selection: agent.desired_selection(),
            relaunch: false,
            execution: crate::runtime::worker::ExecutionHandle::new(
                &fixture.hub,
                "agent",
                "execution",
            ),
            state: super::super::SessionState::from(&agent),
            rpc: &mut transport.session,
            shutdown: &AtomicBool::new(false),
            notify: &Notify::new(),
            turns: &Arc::new(Semaphore::new(1)),
            permit: &mut None,
            idle_seconds: 60,
            thread_id: "thread",
            idle_at: Instant::now(),
            stop_deadline: None,
            interrupt_sent: false,
            admission: None,
            active: None,
            pending_steering: vec![],
            #[cfg(feature = "comb")]
            pending_comb: Vec::new(),
        };
        check(&mut driver);
    }
    transport.finish().await;
    fixture.finish();
}

fn message() -> Message {
    Message {
        has_attachments: false,
        attachments: vec![],
        joined: false,
        id: "message".into(),
        sender: "human".into(),
        recipient: "agent".into(),
        body: "body".into(),
        state: MessageState::Submitting,
    }
}
fn pending(deadline: Option<Instant>) -> steering::Pending {
    steering::Pending {
        message: message(),
        turn: "turn".into(),
        queued: Arc::new(AtomicBool::new(true)),
        reply: Box::pin(std::future::pending()),
        completion: None,
        deadline,
    }
}

#[tokio::test]
async fn idle_expiry_overrides_stop_but_active_and_steering_use_the_earliest_deadline() {
    with_session(|driver| {
        let base = driver.idle_at;
        let stop = base + Duration::from_secs(10);
        driver.stop_deadline = Some(stop);
        assert_eq!(driver.deadline(), base + Duration::from_secs(60));
        driver.idle_seconds = 0;
        assert_eq!(driver.deadline(), stop);
        driver.active = Some(ActiveTurn {
            message: message(),
            turn_id: "turn".into(),
            started: base,
        });
        assert_eq!(driver.deadline(), stop);
        driver.stop_deadline = Some(base + TURN_TIMEOUT + Duration::from_secs(1));
        assert_eq!(driver.deadline(), base + TURN_TIMEOUT);
        let steering = base + Duration::from_secs(5);
        driver.pending_steering = vec![pending(None), pending(Some(steering))];
        assert_eq!(driver.deadline(), steering);
        driver.active = None;
        driver.stop_deadline = None;
        driver.idle_seconds = 60;
        assert_eq!(driver.deadline(), steering);
        driver.pending_steering = vec![pending(None)];
        let before = Instant::now() + NO_DEADLINE_WAIT;
        let deadline = driver.deadline();
        let after = Instant::now() + NO_DEADLINE_WAIT;
        assert!((before..=after).contains(&deadline));
    })
    .await;
}
