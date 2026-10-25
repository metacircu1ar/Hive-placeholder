//! Encode session operations and validate Claude control replies.
use super::limits::{REPLY_ECHO_BYTES, REPLY_ECHO_LIMIT};
use super::{
    ClientMethod, Codec, Error, Result, Turn, Uuid, Value, catalog, parse, protocol, wire,
};
use crate::runtime::provider::claude::SteeringPermit;
use crate::runtime::provider::protocol::codes;
use crate::runtime::provider::protocol::outgoing;
use crate::runtime::steering_canary;

use super::wire::{control, emitted};
use crate::runtime::api;

impl Codec {
    pub(super) fn control(
        &mut self,
        id: u64,
        method: ClientMethod,
        request: control::NativeRequest,
    ) -> Vec<Value> {
        let request_id = format!("hive-{id}");
        self.callbacks
            .controls
            .insert(request_id.clone(), (id, method));
        vec![api::value(control::OutboundControl {
            kind: "control_request",
            request_id: &request_id,
            request,
        })]
    }
    // --replay-user-messages also replays replies to native control requests.
    // Only a byte-value-equivalent reply we emitted can settle an echo; it is
    // never interpreted as an acknowledgement of one of our control requests.
    pub(super) fn remember_replies(&mut self, frames: &[Value]) -> Result<()> {
        let additions = frames
            .iter()
            .filter(|frame| {
                frame["type"].as_str().map(wire::FrameKind::parse)
                    == Some(wire::FrameKind::ControlResponse)
                    && frame["response"]["subtype"]
                        .as_str()
                        .map(wire::ControlSubtype::parse)
                        == Some(wire::ControlSubtype::Success)
            })
            .map(|frame| (frame.clone(), frame.to_string().len()));
        if !self.echoes.pending.try_extend(
            additions,
            super::super::super::queue::Limits {
                items: REPLY_ECHO_LIMIT,
                bytes: REPLY_ECHO_BYTES,
            },
        ) {
            return Err(Error::output_limit("Too many pending provider replies."));
        }
        Ok(())
    }
    pub(crate) fn send(&mut self, frame: outgoing::OutgoingFrame) -> Result<Vec<Value>> {
        let output = self.send_inner(frame)?;
        self.remember_replies(&output)?;
        Ok(output)
    }
    pub(super) fn send_inner(&mut self, frame: outgoing::OutgoingFrame) -> Result<Vec<Value>> {
        use outgoing::{OutgoingFrame, OutgoingRequest};
        let (id, request) = match frame {
            OutgoingFrame::Response(response) => return self.tool_result(response),
            OutgoingFrame::Initialized { .. } => return Ok(vec![]),
            OutgoingFrame::Request { id, request } => (id, request),
        };
        let method = request.method();
        match request {
            OutgoingRequest::Initialize(_) => {
                Ok(self.control(id, method, control::NativeRequest::initialize()))
            }
            OutgoingRequest::ModelList(_) => {
                let catalog = catalog(self.initialization.as_ref().ok_or_else(protocol)?)?;
                self.reply(id, catalog);
                Ok(vec![])
            }
            OutgoingRequest::ThreadStart(_) | OutgoingRequest::ThreadResume(_) => {
                if self.opened || !self.selected_account() {
                    return Err(protocol());
                }
                Ok(self.control(id, method, control::NativeRequest::GetSettings))
            }
            OutgoingRequest::TurnStart(params) => self.start_turn(id, params),
            OutgoingRequest::TurnSteer(params) => self.hold_steering(id, params),
            OutgoingRequest::TurnInterrupt(params) => Ok(self.interrupt_turn(id, method, &params)),
            _ => {
                self.frames.push_back(emitted::error(
                    id,
                    codes::METHOD_NOT_FOUND,
                    "Unsupported native Claude operation.",
                ));
                Ok(vec![])
            }
        }
    }
    fn interrupt_turn(
        &mut self,
        id: u64,
        method: ClientMethod,
        params: &outgoing::Interrupt,
    ) -> Vec<Value> {
        let Some(active) = self.turn.active.as_mut() else {
            self.frames
                .push_back(emitted::error(id, codes::SERVER_ERROR, "No active turn."));
            return vec![];
        };
        if params.turn_id != active.id {
            self.frames.push_back(emitted::error(
                id,
                codes::SERVER_ERROR,
                "Turn no longer active.",
            ));
            return vec![];
        }
        active.interrupt_sent = true;
        self.stale_held();
        self.control(
            id,
            method,
            control::NativeRequest::Interrupt {
                cancel_queued: true,
            },
        )
    }
    pub(super) fn start_turn(&mut self, id: u64, input: outgoing::TurnStart) -> Result<Vec<Value>> {
        self.start_turn_bounded(id, input, crate::limits::JOIN_TRANSPORT_BYTES)
    }
    pub(in crate::runtime::provider::claude) fn start_turn_bounded(
        &mut self,
        id: u64,
        input: outgoing::TurnStart,
        frame_limit: usize,
    ) -> Result<Vec<Value>> {
        if self.turn.input.rejected
            || !self.opened
            || self.turn.active.is_some()
            || !self.tools.active.is_empty()
        {
            return Err(protocol());
        }
        let turn = Uuid::new_v4().to_string();
        let frame = user_frame(&self.config.id, &turn, input.input)?;
        // Reject before recording any submission evidence. Attachment preparation
        // chooses reference-only fallback against this same encoded-frame limit.
        if frame.to_string().len() > frame_limit {
            return Err(protocol());
        }
        // Monotonic for this native process, including partially written prompts.
        self.turn.input.enqueued = true;
        self.branch_input();
        self.turn.terminal_error = false;
        self.turn.assistant_text = None;
        self.turn.active = Some(Turn {
            id: turn.clone(),
            request: id,
            acknowledged: false,
            interrupt_sent: false,
        });
        Ok(vec![frame])
    }
    pub(super) fn control_result(&mut self, result: wire::ControlResult) -> Result<()> {
        let (id, method) = self
            .callbacks
            .controls
            .remove(&result.request_id)
            .ok_or_else(protocol)?;
        if result.subtype != wire::ControlSubtype::Success {
            self.frames.push_back(emitted::error(
                id,
                codes::SERVER_ERROR,
                "Native Claude control request failed.",
            ));
            return Ok(());
        }
        if method == ClientMethod::Initialize {
            self.initialization = Some(parse(result.response)?);
        } else if matches!(
            method,
            ClientMethod::ThreadStart | ClientMethod::ThreadResume
        ) {
            return self.apply_settings(id, result.response);
        } else if method == ClientMethod::TurnInterrupt {
            let receipt: wire::InterruptReceipt = parse(if result.response.is_null() {
                wire::mcp::empty()
            } else {
                result.response
            })?;
            if receipt.still_queued.is_some_and(|ids| !ids.is_empty())
                || receipt.cancelled.is_some_and(|ids| !ids.is_empty())
            {
                // We interrupt only replay-ACKed input; native has already removed
                // its UUID from the cancellable queue. Cancellation here could
                // omit the terminal result, so fail rather than wait indefinitely.
                return Err(protocol());
            }
        }
        self.reply(id, wire::mcp::empty());
        Ok(())
    }
    fn apply_settings(&mut self, id: u64, response: Value) -> Result<()> {
        let settings: wire::Settings = parse(response)?;
        let effort = settings.applied.effort.as_deref().unwrap_or("none");
        let capability = if let Some(candidate) = &self.config.capability {
            candidate.selection(settings.applied.model.as_deref(), effort)
        } else {
            steering_canary::CapabilitySelection::Disabled(
                steering_canary::SteeringObservation::ProofMissing,
            )
        };
        if let Some(mode) = capability.mode() {
            self.steering_permit = Some(SteeringPermit::Verified(mode));
        }
        self.opened = true;
        // Session ID is a reservation; configuration comes from native applied settings.
        self.reply(
            id,
            api::value(emitted::ThreadSettings {
                thread: emitted::Identity {
                    id: &self.config.id,
                },
                model: settings.applied.model.as_deref(),
                reasoning_effort: effort,
                sandbox: self.config.sandbox.as_str(),
                steering_disabled_reason: if self.steering_permit.is_some() {
                    None
                } else {
                    Some(capability.reason())
                },
                steering_proof_error_code: self
                    .config
                    .capability
                    .as_ref()
                    .and_then(steering_canary::Candidate::error_code),
            }),
        );
        Ok(())
    }
}

pub(in crate::runtime) fn user_frame(
    session: &str,
    turn: &str,
    input: Vec<outgoing::TurnInput>,
) -> Result<Value> {
    Ok(api::value(emitted::User {
        kind: "user",
        uuid: turn,
        session_id: session,
        message: emitted::UserText {
            role: "user",
            content: user_content(input)?,
        },
    }))
}

fn user_content(input: Vec<outgoing::TurnInput>) -> Result<emitted::UserContent> {
    let images = input
        .iter()
        .any(|item| matches!(item, outgoing::TurnInput::LocalImage { .. }));
    if images {
        input
            .into_iter()
            .map(|item| match item {
                outgoing::TurnInput::Text { text } => Ok(emitted::UserBlock::Text { text }),
                outgoing::TurnInput::LocalImage { claude, .. } => Ok(emitted::UserBlock::Image {
                    source: claude.ok_or_else(protocol)?,
                }),
            })
            .collect::<Result<Vec<_>>>()
            .map(emitted::UserContent::Blocks)
    } else {
        let text = input
            .into_iter()
            .filter_map(|item| match item {
                outgoing::TurnInput::Text { text } => Some(text),
                outgoing::TurnInput::LocalImage { .. } => None,
            })
            .collect::<Vec<_>>();
        Ok(emitted::UserContent::Text(text.join("\n")))
    }
}
