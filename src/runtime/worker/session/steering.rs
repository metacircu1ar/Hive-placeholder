//! Turn-bound steering submission, receipts and stale-turn outcomes.
use super::{
    Agent, Error, EventKind, Message, MessageState, Ordering, Response, Result, Session,
    SessionLoop, outgoing,
};
use crate::runtime::Core;
use crate::runtime::api::{self, events};
use std::future::poll_fn;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::task::Poll;
use std::time::{Duration, Instant};

// Bound both normal-lane backpressure and acknowledgement, including during stop.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(30);
// The public timeout message in turn_loop states this duration in seconds.
const _: () = assert!(DELIVERY_TIMEOUT.as_secs() == 30);

/// Probe an idle, bound session with valid input and an impossible expected turn.
/// The turn precondition prevents submission regardless of input-validation order.
pub(super) async fn probe(rpc: &Session, thread: &str) -> Result<bool> {
    let expected = uuid::Uuid::new_v4().to_string();
    match rpc
        .call(outgoing::steer(
            thread,
            &expected,
            "Hive steering capability probe.",
        ))
        .await
    {
        Err(Error::NoActiveTurn { .. }) => Ok(true),
        Err(
            Error::ProviderRequestFailed { .. }
            | Error::UnsupportedOperation { .. }
            | Error::SteeringUncertain { .. },
        ) => Ok(false),
        Err(Error::StaleTurn { .. }) => {
            rpc.quarantine(thread).await?;
            Err(Error::provider_protocol_error(
                "Provider reported an untracked active turn during the steering probe.",
            ))
        }
        Err(error) => Err(error),
        Ok(_) => {
            rpc.quarantine(thread).await?;
            Err(Error::provider_protocol_error(
                "Provider accepted an idle steering capability probe.",
            ))
        }
    }
}

pub(super) struct Pending {
    message: Message,
    queued: std::sync::Arc<std::sync::atomic::AtomicBool>,
    turn: String,
    reply: std::pin::Pin<Box<dyn std::future::Future<Output = Result<Response>> + Send>>,
    pub(super) completion: Option<MessageState>,
    pub(super) deadline: Option<Instant>,
}

pub(super) struct Acknowledgement {
    pub index: usize,
    pub result: Result<Response>,
}
pub(super) async fn acknowledgement(pending: &mut [Pending]) -> Acknowledgement {
    poll_fn(|cx| {
        for (index, request) in pending.iter_mut().enumerate() {
            if let Poll::Ready(result) = request.reply.as_mut().poll(cx) {
                return Poll::Ready(Acknowledgement { index, result });
            }
        }
        Poll::Pending
    })
    .await
}

pub(super) fn queued(core: &Core, agent: &Agent) -> Result<Option<Message>> {
    if !agent.steering.supported()
        || agent.paused
        || agent.terminal()
        || agent.control.is_some()
        || core.shutdown().load(Ordering::Relaxed)
    {
        return Ok(None);
    }
    match (&agent.execution_id, &agent.turn_id) {
        (Some(execution), Some(turn)) => core.store().next_steering(&agent.id, execution, turn),
        _ => Ok(None),
    }
}

impl SessionLoop<'_> {
    // Preserve already-returned exact receipts on an interrupt/error exit. Never
    // poll an unqueued future here: that could submit new guidance during cleanup.
    pub(super) async fn settle_ready_steering(&mut self) -> Result<()> {
        loop {
            let ready = poll_fn(|cx| {
                for (index, pending) in self.pending_steering.iter_mut().enumerate() {
                    if pending.queued.load(Ordering::Acquire)
                        && let Poll::Ready(result) = pending.reply.as_mut().poll(cx)
                    {
                        return Poll::Ready(Some((index, result)));
                    }
                }
                Poll::Ready(None)
            })
            .await;
            let Some((index, result)) = ready else {
                return Ok(());
            };
            self.finish_steering(index, result).await?;
        }
    }

    #[expect(
        clippy::expect_used,
        reason = "read_turn fetches guidance only for an active turn; the loop handles it \
            before another frame can complete that turn."
    )]
    pub(super) async fn start_steering(&mut self, message: Message) -> Result<()> {
        let turn = self
            .active
            .as_ref()
            .expect("steering requires active turn")
            .turn_id
            .clone();
        let target = turn.clone();
        let message = self
            .execution
            .agent(move |core, agent| {
                // Controls may have changed since the main loop read the inbox.
                if agent.paused
                    || agent.terminal()
                    || agent.control.is_some()
                    || agent.turn_id.as_deref() != Some(&target)
                    || core.shutdown().load(Ordering::Relaxed)
                {
                    return Ok(None);
                }
                if !core.store().steering_state(
                    &message,
                    MessageState::Accepted,
                    MessageState::Submitting,
                    None,
                )? {
                    return Ok(None);
                }
                Ok(Some(message))
            })
            .await?;
        let Some(message) = message else {
            return Ok(());
        };
        let input = message.input();
        let queued = Arc::new(AtomicBool::new(false));
        let reply = Box::pin(self.rpc.observed_call(
            outgoing::steer(self.thread_id, &turn, &input),
            Some(queued.clone()),
        ));
        self.pending_steering.push(Pending {
            message,
            queued,
            turn,
            reply,
            completion: None,
            deadline: self.rpc.codex().then(|| Instant::now() + DELIVERY_TIMEOUT),
        });
        Ok(())
    }

    pub(super) async fn finish_steering(
        &mut self,
        index: usize,
        result: Result<Response>,
    ) -> Result<()> {
        let Pending {
            message,
            turn,
            completion,
            ..
        } = self.pending_steering.remove(index);
        let evidence = result.as_ref().ok().map(|r| r.clone().into_raw());
        let (state, error) = match result {
            Ok(_) => (completion.unwrap_or(MessageState::Submitted), None),
            Err(error @ (Error::NoActiveTurn { .. } | Error::StaleTurn { .. })) => {
                (MessageState::StaleTurn, Some(error))
            }
            Err(
                error @ (Error::UnsupportedOperation { .. } | Error::ProviderRequestFailed { .. }),
            ) => (MessageState::Failed, Some(error)),
            Err(error @ Error::SteeringUncertain { .. }) => (MessageState::Uncertain, Some(error)),
            // Submission may have crossed the pipe. The worker's failure path
            // marks submitting input uncertain and performs owned cleanup.
            Err(error) => {
                self.rpc.drain();
                return Err(error);
            }
        };
        self.execution
            .agent(move |core, agent| {
                if !core.store().steering_state(
                    &message,
                    MessageState::Submitting,
                    state,
                    error.as_ref().map(Error::code),
                )? {
                    return Ok(());
                }
                if matches!(error, Some(Error::UnsupportedOperation { .. })) {
                    agent.disable_steering();
                    core.store().save(agent)?;
                }
                core.store().event(
                    agent,
                    if error.is_none() {
                        EventKind::SteeringSubmitted
                    } else if state == MessageState::Uncertain {
                        EventKind::SteeringUncertain
                    } else {
                        EventKind::SteeringRejected
                    },
                    api::value(events::SteeringOutcome {
                        message_id: &message.id,
                        turn_id: &turn,
                        state,
                        error: error.as_ref().map(Error::code),
                        receipt: evidence.as_ref(),
                    }),
                )
            })
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod deadline_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{provider::WorkerTransport, test_support::RuntimeFixture};
    use serde_json::json;
    use std::sync::{Arc, atomic::AtomicBool};
    use tokio::sync::{Notify, Semaphore};

    fn queued_message(fixture: &RuntimeFixture) -> (Agent, Message) {
        fixture
            .hub
            .blocking_with(|core| {
                let mut agent = core.store().agent("agent")?;
                agent.steering = crate::runtime::domain::SteeringCapability::Supported;
                core.store().save(&agent)?;
                core.store()
                    .enqueue_steering("guidance", "human", &agent, "marker")?;
                let message = core
                    .store()
                    .next_steering("agent", "execution", "turn")?
                    .unwrap();
                core.store()
                    .message_state(&message, MessageState::Submitting)?;
                Ok((agent, message))
            })
            .unwrap()
    }
    #[test]
    fn cleanup_preserves_ready_hook_receipt_without_polling_unqueued_guidance() {
        let fixture = RuntimeFixture::new();
        let (agent, message) = queued_message(&fixture);
        let receipt = json!({"turnId":"turn","deliveryEvidence":{"kind":"written_continuation",
            "observation":"verified_assistant_content","hookReply":{"type":"control_response","response":{
            "subtype":"success","request_id":"post-hook","response":{"hookSpecificOutput":{"hookEventName":"PostToolUse","additionalContext":"marker"}}}}}});
        let observed = receipt.clone();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut transport = WorkerTransport::new(&fixture, json!({}));
                let notify = Notify::new();
                let shutdown = AtomicBool::new(false);
                let turns = Arc::new(Semaphore::new(1));
                let mut permit = None;
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
                    shutdown: &shutdown,
                    notify: &notify,
                    turns: &turns,
                    permit: &mut permit,
                    idle_seconds: 0,
                    thread_id: "thread",
                    idle_at: Instant::now(),
                    stop_deadline: None,
                    interrupt_sent: false,
                    admission: None,
                    active: None,
                    #[cfg(feature = "comb")]
                    pending_comb: Vec::new(),
                    pending_steering: vec![
                        unqueued_guidance(),
                        Pending {
                            message,
                            turn: "turn".into(),
                            queued: Arc::new(AtomicBool::new(true)),
                            reply: Box::pin(async move { Ok(Response::parse(observed)) }),
                            completion: Some(MessageState::Completed),
                            deadline: None,
                        },
                    ],
                };
                driver.settle_ready_steering().await.unwrap();
                assert_eq!(driver.pending_steering.len(), 1);
                drop(driver);
                transport.finish().await;
            });
        let messages = fixture
            .hub
            .blocking_with(|core| core.store().messages("agent", None, 200))
            .unwrap();
        let guidance = messages["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["id"] == "guidance")
            .unwrap();
        assert_eq!(guidance["state"], "completed");
        let events = fixture.events();
        assert!(
            events["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|event| event["data"]["receipt"] == receipt)
        );
        fixture.finish();
    }
    fn unqueued_guidance() -> Pending {
        Pending {
            message: Message {
                has_attachments: false,
                attachments: vec![],
                joined: false,
                id: "unqueued".into(),
                sender: "human".into(),
                body: "held".into(),
                recipient: "agent".into(),
                state: MessageState::Submitting,
            },
            turn: "turn".into(),
            queued: Arc::new(AtomicBool::new(false)),
            reply: Box::pin(async { panic!("cleanup must not enqueue guidance") }),
            completion: None,
            deadline: None,
        }
    }
}
