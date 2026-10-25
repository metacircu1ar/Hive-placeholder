//! Message admission, steering and redirection.
use super::{AgentState, Caller, Core, Error, EventKind, MessageState, Result, eligible};
use crate::runtime::api::InputContext as Op;
use crate::runtime::{api::events, domain::DeliveryMode};
use crate::{runtime::api, state};
use serde_json::Value;
use uuid::Uuid;

impl Core {
    pub(super) fn send(
        &mut self,
        caller: &Caller,
        args: &api::SendArgs,
        attachments: &[crate::runtime::attachments::Attachment],
    ) -> Result<Value> {
        let delay = crate::runtime::scheduled::delay(args)?;
        let not_before = delay
            .map(|n| crate::runtime::scheduled::not_before(state::wall_time(), n))
            .transpose()?;
        let mode = args.delivery_mode()?;
        let steering_files;
        let attachments = if mode == Some(DeliveryMode::Steer) {
            steering_files = steering_attachments(attachments);
            &steering_files
        } else {
            attachments
        };
        let target = args.target.required(Op::Send, "/target")?;
        let body = args.body.required(Op::Send, "/body")?;
        let mut agent = self.store.relative_target(caller, target, Op::Send)?;
        let target = agent.id.clone();
        eligible(&agent)?;
        let steering = mode == Some(crate::runtime::domain::DeliveryMode::Steer);
        let redirect = mode == Some(crate::runtime::domain::DeliveryMode::Redirect);
        let steering_target = steering.then(|| check_steering(&agent)).transpose()?;
        if super::super::features::available(&agent)
            && not_before.is_none()
            && !steering
            && agent.state == AgentState::Stopped
            && !agent.paused
        {
            agent.state = AgentState::Starting;
            self.store.save(&agent)?;
        }
        let message_id = Uuid::new_v4().to_string();
        let sender_turn_id = if caller.is_human() {
            None
        } else {
            self.store.agent(caller.name())?.turn_id
        };
        let redirect_target = Delivery {
            target: &target,
            not_before,
            steering,
            redirect,
        }
        .enqueue(self, caller, &mut agent, &message_id, body)?;
        self.store.attach(&message_id, attachments)?;
        if caller.is_human() && !steering {
            self.store.track_call(&message_id)?;
        }
        MessageEvidence {
            attachments,
            message_id: &message_id,
            caller,
            body,
            sender_turn_id,
            not_before,
            steering_target: &steering_target,
            redirect,
            redirect_target: &redirect_target,
        }
        .record(self, &agent)?;
        Ok(api::value(api::SendResponse {
            held_for_build: None,
            message_id,
            delivery_state: if not_before.is_some() {
                MessageState::Scheduled
            } else {
                MessageState::Accepted
            },
            not_before,
            recipient: &target,
            steering_target,
            redirect_target,
        }))
    }
}

fn check_steering(agent: &super::Agent) -> Result<api::SteeringTarget> {
    if agent.paused || agent.state != AgentState::Running || agent.control.is_some() {
        return Err(Error::no_active_turn(
            "Steering requires an active, unpaused turn.",
        ));
    }
    let target = api::SteeringTarget::from_agent(agent).ok_or(Error::no_active_turn(
        "Steering requires an active, unpaused turn.",
    ))?;
    if !agent.steering.supported() {
        return Err(Error::unsupported_operation(
            "This provider session does not support steering.",
        ));
    }
    Ok(target)
}

struct Delivery<'a> {
    target: &'a str,
    not_before: Option<u64>,
    steering: bool,
    redirect: bool,
}
impl Delivery<'_> {
    fn enqueue(
        &self,
        core: &Core,
        caller: &Caller,
        agent: &mut super::Agent,
        message_id: &str,
        body: &str,
    ) -> Result<Option<api::RedirectTarget>> {
        Ok(if let Some(at) = self.not_before {
            core.store
                .enqueue_scheduled(message_id, caller.name(), self.target, body, at)?;
            None
        } else if self.redirect {
            core.store
                .enqueue_redirect(message_id, caller.name(), agent, body)?
        } else if self.steering {
            core.store
                .enqueue_steering(message_id, caller.name(), agent, body)?;
            None
        } else {
            core.store
                .enqueue(message_id, caller.name(), self.target, body)?;
            None
        })
    }
}

struct MessageEvidence<'a> {
    attachments: &'a [crate::runtime::attachments::Attachment],
    message_id: &'a str,
    caller: &'a Caller,
    body: &'a str,
    sender_turn_id: Option<String>,
    not_before: Option<u64>,
    steering_target: &'a Option<api::SteeringTarget>,
    redirect: bool,
    redirect_target: &'a Option<api::RedirectTarget>,
}
impl MessageEvidence<'_> {
    fn record(self, core: &Core, agent: &super::Agent) -> Result<()> {
        let Self {
            attachments,
            message_id,
            caller,
            body,
            sender_turn_id,
            not_before,
            steering_target,
            redirect,
            redirect_target,
        } = self;
        let evidence = api::value(events::MessageAccepted {
            attachments,
            id: message_id,
            sender: caller,
            body,
            sender_turn_id: sender_turn_id.as_deref(),
            not_before,
            mode: if redirect {
                Some(DeliveryMode::Redirect)
            } else {
                steering_target.as_ref().map(|_| DeliveryMode::Steer)
            },
            steering_target: steering_target.as_ref(),
            redirect_target: redirect.then_some(redirect_target),
        });
        core.store.event(
            agent,
            if redirect {
                EventKind::RedirectAccepted
            } else {
                EventKind::MessageAccepted
            },
            evidence,
        )?;
        Ok(())
    }
}

fn steering_attachments(
    attachments: &[crate::runtime::attachments::Attachment],
) -> Vec<crate::runtime::attachments::Attachment> {
    attachments
        .iter()
        .cloned()
        .map(|mut a| {
            a.delivery = crate::runtime::attachments::Delivery::Reference {
                reason: crate::runtime::attachments::ReferenceReason::Steering,
            };
            a
        })
        .collect::<Vec<_>>()
}
