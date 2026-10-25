//! Turn-bound hook delivery. Holding guidance is distinct from writing its receipt.
use super::limits::{REPLY_ECHO_BYTES, REPLY_ECHO_LIMIT};
use super::wire::emitted;
use super::{Codec, protocol};
use crate::error::Result;
use crate::runtime::api;
use crate::runtime::provider::protocol::codes;
use crate::runtime::provider::protocol::outgoing::Steer;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::mem;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

// Bound queued and emitted guidance together so admission cannot overrun codec capacity.
pub(crate) const MAX_HELD: usize = 16;
// Bound JSON-escaped guidance and its separators before creating a native hook reply.
const MAX_BYTES: usize = 2 * 1024 * 1024;
// A written hook reply must gain delivery evidence within this fixed window.
const RECEIPT_TIMEOUT: Duration = Duration::from_secs(30);
// Two newline separators each encode as two JSON bytes.
const GUIDANCE_SEPARATOR_BYTES: usize = 4;
// Retain bounded settled replies for duplicate suppression after generic echo retirement.
const SETTLED_REPLY_BYTES: usize = 4 * MAX_BYTES;
const _: () = assert!(MAX_BYTES < REPLY_ECHO_BYTES);
const _: () = assert!(MAX_HELD <= REPLY_ECHO_LIMIT);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReceiptMode {
    NativeEcho,
    WrittenContinuation,
}

// Only the explicit owned canary or a validated proof can create this permit.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Permit {
    Verified(ReceiptMode),
    Canary,
}

struct Held {
    request: u64,
    turn: String,
    text: String,
    bytes: usize,
}
struct Emitted {
    held: Vec<Held>,
    reply: Value,
    deadline: Instant,
    written: bool,
}
#[derive(Default)]
pub(super) struct Steering {
    held: VecDeque<Held>,
    emitted: Vec<Emitted>,
    // Exact late/duplicate hook echoes must never acknowledge another batch.
    settled: crate::runtime::provider::queue::ByteQueue<Value>,
}

impl Codec {
    pub(super) fn hold_steering(&mut self, id: u64, input: Steer) -> Result<Vec<Value>> {
        if self.steering_permit.is_none() {
            self.frames.push_back(emitted::error(
                id,
                codes::METHOD_NOT_FOUND,
                "Unsupported native Claude operation.",
            ));
            return Ok(vec![]);
        }
        self.check_session(&input.thread_id)?;
        if self
            .turn
            .active
            .as_ref()
            .filter(|t| t.acknowledged && !t.interrupt_sent)
            .is_none_or(|t| t.id != input.expected_turn_id)
            || self.turn.completion.is_some()
        {
            self.steering_reject(id, false);
            return Ok(vec![]);
        }
        let text = input
            .input
            .into_iter()
            .map(|t| t.text)
            .collect::<Vec<_>>()
            .join("\n");
        let bytes = api::value(&text).to_string().len() + GUIDANCE_SEPARATOR_BYTES;
        if self.held_capacity_exceeded(bytes) {
            self.frames.push_back(emitted::error(
                id,
                codes::INVALID_PARAMS,
                "Held steering batch is full; retry after delivery.",
            ));
            return Ok(vec![]);
        }
        self.steering.held.push_back(Held {
            request: id,
            turn: input.expected_turn_id,
            text,
            bytes,
        });
        Ok(vec![])
    }
    fn held_capacity_exceeded(&self, bytes: usize) -> bool {
        let bytes = self
            .steering
            .held
            .iter()
            .chain(
                self.steering
                    .emitted
                    .iter()
                    .flat_map(|batch| batch.held.iter()),
            )
            .map(|h| h.bytes)
            .sum::<usize>()
            + bytes;
        self.steering.held.len()
            + self
                .steering
                .emitted
                .iter()
                .map(|b| b.held.len())
                .sum::<usize>()
            >= MAX_HELD
            || bytes > MAX_BYTES
    }
    pub(super) fn hook_guidance(&mut self) -> Option<Value> {
        let active = self.turn.active.as_ref().map(|t| t.id.clone());
        let held = mem::take(&mut self.steering.held);
        for request in held {
            if active.as_deref() == Some(&request.turn) && self.turn.completion.is_none() {
                self.steering.held.push_back(request);
            } else {
                self.steering_reject(request.request, false);
            }
        }
        if self.turn.completion.is_some()
            || self.steering.held.is_empty()
            || self.turn.active.as_ref().is_none_or(|t| t.interrupt_sent)
        {
            return None;
        }
        Some(api::value(emitted::Guidance {
            hook_specific_output: emitted::Context {
                hook_event_name: "PostToolUse",
                additional_context: self
                    .steering
                    .held
                    .iter()
                    .map(|h| h.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            },
        }))
    }
    pub(super) fn emitted_guidance(&mut self, reply: &mut Value) -> Result<()> {
        if reply["response"]["response"]["hookSpecificOutput"]["additionalContext"].is_string() {
            if !self.echoes.pending.fits(
                1,
                reply.to_string().len(),
                crate::runtime::provider::queue::Limits {
                    items: REPLY_ECHO_LIMIT,
                    bytes: REPLY_ECHO_BYTES,
                },
            ) {
                for held in mem::take(&mut self.steering.held) {
                    self.frames.push_back(emitted::error(
                        held.request,
                        codes::INVALID_PARAMS,
                        "Hook reply capacity is occupied; guidance was not emitted.",
                    ));
                }
                reply["response"]["response"] = super::wire::mcp::empty();
                return Ok(());
            }
            if self.steering.emitted.len() >= MAX_HELD {
                return Err(protocol());
            }
            self.steering.emitted.push(Emitted {
                held: self.steering.held.drain(..).collect(),
                reply: reply.clone(),
                deadline: Instant::now() + RECEIPT_TIMEOUT,
                written: false,
            });
        }
        Ok(())
    }
    pub(crate) fn written(&mut self, reply: &Value) {
        for batch in &mut self.steering.emitted {
            if batch.reply == *reply {
                batch.written = true;
            }
        }
    }
    pub(super) fn steering_echo(&mut self, reply: &Value) -> bool {
        if let Some(index) = self.steering.emitted.iter().position(|b| b.reply == *reply) {
            // A complete echo itself proves the native peer received the full reply.
            if !matches!(
                self.steering_permit,
                Some(Permit::Verified(ReceiptMode::WrittenContinuation))
            ) {
                self.confirm_guidance(index, ReceiptMode::NativeEcho, "native_echo");
            }
            return true;
        }
        self.steering.settled.iter().any(|settled| settled == reply)
    }
    pub(super) fn steering_continuation(&mut self) {
        self.confirm_written("verified_assistant_content");
    }
    pub(super) fn steering_success(&mut self) {
        if self
            .turn
            .active
            .as_ref()
            .is_some_and(|turn| !turn.interrupt_sent)
        {
            self.confirm_written("successful_terminal_result");
        }
    }
    fn confirm_written(&mut self, observation: &str) {
        if self.turn.completion.is_some()
            || !matches!(
                self.steering_permit,
                Some(Permit::Verified(ReceiptMode::WrittenContinuation) | Permit::Canary)
            )
        {
            return;
        }
        let Some(turn) = self.turn.active.as_ref().map(|t| t.id.clone()) else {
            return;
        };
        while let Some(index) = self
            .steering
            .emitted
            .iter()
            .position(|b| b.written && b.held.iter().all(|h| h.turn == turn))
        {
            self.confirm_guidance(index, ReceiptMode::WrittenContinuation, observation);
        }
    }
    fn confirm_guidance(&mut self, index: usize, mode: ReceiptMode, observation: &str) {
        let batch = self.steering.emitted.remove(index);
        self.remember_steering_reply(batch.reply.clone());
        for held in batch.held {
            self.reply(
                held.request,
                api::value(emitted::SteeringDelivery {
                    turn_id: &held.turn,
                    delivery_evidence: emitted::DeliveryEvidence {
                        kind: mode,
                        observation,
                        hook_reply: &batch.reply,
                    },
                }),
            );
        }
    }
    fn remember_steering_reply(&mut self, reply: Value) {
        // Steering now owns bounded duplicate suppression. No native echo is
        // required in continuation mode, so retire the generic ledger charge.
        self.echoes.pending.retain(|entry| *entry != reply);
        let bytes = reply.to_string().len();
        self.steering.settled.push_back(reply, bytes);
        while self.steering.settled.len() > MAX_HELD
            || self.steering.settled.bytes() > SETTLED_REPLY_BYTES
        {
            self.steering.settled.pop_front();
        }
    }
    fn steering_reject(&mut self, request: u64, uncertain: bool) {
        self.frames.push_back(if uncertain {
            emitted::error(request, codes::SERVER_ERROR, "Hook delivery is uncertain.")
        } else {
            emitted::error(request, codes::INVALID_REQUEST, "no active turn to steer")
        });
    }
    pub(super) fn stale_held(&mut self) {
        for held in mem::take(&mut self.steering.held) {
            self.steering_reject(held.request, false);
        }
    }
    pub(super) fn expire_steering(&mut self, end: bool, cancelled: Option<&str>) {
        if end {
            self.stale_held();
        }
        let mut index = 0;
        while index < self.steering.emitted.len() {
            let b = &self.steering.emitted[index];
            if end
                || (self.receipt_timed(b) && Instant::now() >= b.deadline)
                || cancelled.is_some_and(|id| b.reply["response"]["request_id"] == id)
            {
                let batch = self.steering.emitted.remove(index);
                self.remember_steering_reply(batch.reply);
                for held in batch.held {
                    self.steering_reject(held.request, true);
                }
            } else {
                index += 1;
            }
        }
    }
    fn receipt_timed(&self, batch: &Emitted) -> bool {
        !batch.written
            || matches!(
                self.steering_permit,
                Some(Permit::Verified(ReceiptMode::NativeEcho))
            )
    }
    pub(crate) fn steering_wait(&self, limit: Duration) -> Duration {
        self.steering
            .emitted
            .iter()
            .filter(|b| self.receipt_timed(b))
            .map(|b| b.deadline.saturating_duration_since(Instant::now()))
            .min()
            .map_or(limit, |d| d.min(limit))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{runtime::provider::claude::SessionConfig, shell::guard::ShellGuard};
    use serde_json::json;
    use std::sync::Arc;

    #[test]
    fn only_emitted_guidance_gets_a_receipt_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let config = Arc::new(SessionConfig {
            instructions: String::new(),
            steering: Some(Permit::Verified(ReceiptMode::NativeEcho)),
            ..SessionConfig::for_test("session", dir.path().into())
        });
        let launcher = crate::launch::test_cache().unwrap();
        let guard = ShellGuard::create(&config.cwd, &config.instructions, || launcher.executable())
            .unwrap();
        let mut codec = Codec::new(config, guard);
        codec.steering.held.push_back(Held {
            request: 1,
            turn: "turn".into(),
            text: "held".into(),
            bytes: 10,
        });
        codec.steering.emitted.push(Emitted {
            held: vec![Held {
                request: 2,
                turn: "turn".into(),
                text: "sent".into(),
                bytes: 10,
            }],
            reply: json!({}),
            deadline: Instant::now().checked_sub(Duration::from_secs(1)).unwrap(),
            written: true,
        });
        assert_eq!(
            codec.steering_wait(Duration::from_secs(3600)),
            Duration::ZERO
        );
        codec.expire_steering(false, None);
        assert_eq!(codec.pop().unwrap()["id"], 2);
        assert!(codec.pop().is_none());
        assert_eq!(codec.steering.held.len(), 1);
        assert_eq!(
            codec.steering_wait(Duration::from_secs(3600)),
            Duration::from_secs(3600)
        );
        codec.expire_steering(true, None);
        assert_eq!(codec.pop().unwrap()["id"], 1);
    }
    #[test]
    fn written_continuation_waits_for_model_without_receipt_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let config = Arc::new(SessionConfig {
            instructions: String::new(),
            steering: Some(Permit::Verified(ReceiptMode::WrittenContinuation)),
            ..SessionConfig::for_test("session", dir.path().into())
        });
        let launcher = crate::launch::test_cache().unwrap();
        let guard = ShellGuard::create(&config.cwd, &config.instructions, || launcher.executable())
            .unwrap();
        let mut codec = Codec::new(config, guard);
        codec.turn.active = Some(super::super::Turn {
            id: "turn".into(),
            request: 1,
            acknowledged: true,
            interrupt_sent: false,
        });
        codec.steering.emitted.push(Emitted {
            held: vec![Held {
                request: 2,
                turn: "turn".into(),
                text: "sent".into(),
                bytes: 10,
            }],
            reply: json!({}),
            deadline: Instant::now().checked_sub(Duration::from_secs(40)).unwrap(),
            written: true,
        });
        codec.expire_steering(false, None);
        assert!(codec.pop().is_none());
        codec.steering_continuation();
        assert_eq!(codec.pop().unwrap()["id"], 2);
        codec.steering.emitted.push(Emitted {
            held: vec![Held {
                request: 3,
                turn: "turn".into(),
                text: "partial".into(),
                bytes: 13,
            }],
            reply: json!({}),
            deadline: Instant::now().checked_sub(Duration::from_secs(40)).unwrap(),
            written: false,
        });
        codec.expire_steering(false, None);
        assert_eq!(codec.pop().unwrap()["error"]["code"], -32000);
    }
    #[test]
    fn pending_reply_budget_rejects_guidance_before_emission() {
        let dir = tempfile::tempdir().unwrap();
        let config = Arc::new(SessionConfig {
            instructions: String::new(),
            steering: Some(Permit::Verified(ReceiptMode::NativeEcho)),
            ..SessionConfig::for_test("session", dir.path().into())
        });
        let launcher = crate::launch::test_cache().unwrap();
        let guard = ShellGuard::create(&config.cwd, &config.instructions, || launcher.executable())
            .unwrap();
        let mut codec = Codec::new(config, guard);
        let old = json!({"type":"control_response","response":{"subtype":"success","request_id":"old","response":"x".repeat(3*1024*1024)}});
        codec.remember_replies(std::slice::from_ref(&old)).unwrap();
        // Seed the retained byte charge without allocating the whole transport budget.
        codec.echoes.pending.clear();
        codec
            .echoes
            .pending
            .push_back(old, REPLY_ECHO_BYTES - 1024 * 1024);
        codec.steering.held.push_back(Held {
            request: 2,
            turn: "turn".into(),
            text: "new".into(),
            bytes: 9,
        });
        let mut reply = json!({"type":"control_response","response":{"subtype":"success","request_id":"new","response":{
            "hookSpecificOutput":{"additionalContext":"x".repeat(2*1024*1024)}}}});
        codec.emitted_guidance(&mut reply).unwrap();
        assert!(
            reply["response"]["response"]
                .as_object()
                .unwrap()
                .is_empty()
        );
        assert_eq!(codec.pop().unwrap()["error"]["code"], -32602);
        assert!(codec.steering.emitted.is_empty());
        codec.remember_replies(&[reply]).unwrap();
    }
    #[test]
    fn repeated_no_echo_success_retires_generic_reply_charges() {
        let dir = tempfile::tempdir().unwrap();
        let config = Arc::new(SessionConfig {
            instructions: String::new(),
            steering: Some(Permit::Verified(ReceiptMode::WrittenContinuation)),
            ..SessionConfig::for_test("session", dir.path().into())
        });
        let launcher = crate::launch::test_cache().unwrap();
        let guard = ShellGuard::create(&config.cwd, &config.instructions, || launcher.executable())
            .unwrap();
        let mut codec = Codec::new(config, guard);
        codec.turn.active = Some(super::super::Turn {
            id: "turn".into(),
            request: 1,
            acknowledged: true,
            interrupt_sent: false,
        });
        for id in 2..302 {
            codec.steering.held.push_back(Held {
                request: id,
                turn: "turn".into(),
                text: "held".into(),
                bytes: 10,
            });
            let mut reply = json!({"type":"control_response","response":{"subtype":"success","request_id":id.to_string(),
                "response":{"hookSpecificOutput":{"additionalContext":"held"}}}});
            codec.emitted_guidance(&mut reply).unwrap();
            codec.remember_replies(&[reply.clone()]).unwrap();
            codec.written(&reply);
            codec.steering_continuation();
            assert_eq!(codec.pop().unwrap()["id"], id);
            assert!(codec.echoes.pending.is_empty());
            assert!(codec.steering.settled.len() <= MAX_HELD);
            assert!(codec.steering_echo(&reply));
        }
    }
}
