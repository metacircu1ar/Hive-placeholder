//! Track assistant text and native tool activity within the active turn.
use super::limits::{ACTIVE_TOOL_LIMIT, SEEN_TOOL_LIMIT, TOOL_ID_BYTES, TOOL_NAME_BYTES};
use super::{
    ASSISTANT_TEXT_LIMIT, AssistantText, Codec, Error, Result, ServerMethod, ToolOwner, protocol,
    wire,
};

use super::wire::emitted;
use crate::runtime::api;

struct AssistantOutput {
    method: ServerMethod,
    item: emitted::AssistantItem,
}

impl Codec {
    pub(super) fn observe_tool(&mut self, id: &str, name: &str, pre: bool) -> Result<()> {
        let turn = self.active_id()?;
        // At most 8 MiB of lifetime ID text. Do not evict IDs: hooks lack a turn
        // discriminator. Permit either observation order exactly once per tool.
        if id.is_empty() || id.len() > TOOL_ID_BYTES || name.len() > TOOL_NAME_BYTES {
            return Err(protocol());
        }
        if let Some(owner) = self.tools.owners.get_mut(id) {
            if owner.turn != turn
                || owner.name != name
                || if pre {
                    owner.pre_seen
                } else {
                    owner.assistant_seen
                }
            {
                return Err(protocol());
            }
            if pre {
                owner.pre_seen = true;
            } else {
                owner.assistant_seen = true;
            }
            return Ok(());
        }
        if self.tools.owners.len() >= ACTIVE_TOOL_LIMIT
            || self.tools.seen.len() >= SEEN_TOOL_LIMIT
            || !self.tools.seen.insert(id.into())
        {
            return Err(protocol());
        }
        self.tools.owners.insert(
            id.into(),
            ToolOwner {
                turn,
                name: name.into(),
                allowed: false,
                assistant_seen: !pre,
                pre_seen: pre,
            },
        );
        Ok(())
    }
    pub(super) fn assistant(
        &mut self,
        message: wire::Assistant,
        session: &str,
        parent: Option<String>,
    ) -> Result<()> {
        self.check_session(session)?;
        // A skill may run its fork on another model; only its tool calls count.
        if self.in_fork(parent.as_deref()) && self.turn.completion.is_none() {
            return self.fork_assistant(message);
        }
        let verified = wire::ModelKind::parse(&message.model) != wire::ModelKind::Synthetic;
        if verified {
            self.check_model(&message.model)?;
        }
        if parent.is_some() || self.turn.completion.is_some() {
            return Err(protocol());
        }
        let turn = self.active_id()?;
        if verified && !message.content.is_empty() {
            self.steering_continuation();
        }
        // Native Claude splits a logical assistant message into separate frames.
        // Missing IDs scope text to this frame; synthetic notices never replace
        // the real message being assembled.
        if verified {
            self.begin_assistant_text(&message, &turn);
        }
        let message_id = message.id.clone();
        for block in message.content {
            let Some(AssistantOutput { method, item }) =
                self.assistant_item(block, verified, message_id.as_deref())?
            else {
                continue;
            };
            self.notice(
                method,
                api::value(emitted::ItemParams {
                    thread_id: &self.config.id,
                    turn_id: &turn,
                    item,
                }),
            );
        }
        if verified && message.id.is_none() {
            self.flush_assistant_text();
        }
        Ok(())
    }
    /// A forked skill's assistant frame: its tool calls are tracked and shown
    /// like the agent's own; its text is the subagent's, not the agent's reply.
    fn fork_assistant(&mut self, message: wire::Assistant) -> Result<()> {
        let turn = self.active_id()?;
        for block in message.content {
            if let wire::Block::ToolUse { .. } = block
                && let Some(AssistantOutput { method, item }) =
                    self.assistant_item(block, false, None)?
            {
                self.notice(
                    method,
                    api::value(emitted::ItemParams {
                        thread_id: &self.config.id,
                        turn_id: &turn,
                        item,
                    }),
                );
            }
        }
        Ok(())
    }
    fn assistant_item(
        &mut self,
        block: wire::Block,
        verified: bool,
        message: Option<&str>,
    ) -> Result<Option<AssistantOutput>> {
        Ok(Some(match block {
            wire::Block::Text { text } => {
                if verified {
                    self.append_assistant_text(&text)?;
                }
                AssistantOutput {
                    method: ServerMethod::ItemCompleted,
                    item: emitted::AssistantItem::AgentMessage {
                        id: message.map(str::to_owned),
                        text,
                        verified_model: verified,
                    },
                }
            }
            wire::Block::ToolUse { id, name, input } => {
                self.observe_tool(&id, &name, false)?;
                AssistantOutput {
                    method: ServerMethod::ItemStarted,
                    item: emitted::AssistantItem::DynamicToolCall {
                        id,
                        tool: name,
                        arguments: input,
                    },
                }
            }
            wire::Block::Other => return Ok(None),
        }))
    }
    fn begin_assistant_text(&mut self, message: &wire::Assistant, turn: &str) {
        if self.turn.assistant_text.as_ref().is_some_and(|text| {
            text.turn != turn || text.message != message.id || message.id.is_none()
        }) {
            self.flush_assistant_text();
        }
        self.turn
            .assistant_text
            .get_or_insert_with(|| AssistantText {
                message: message.id.clone(),
                turn: turn.to_owned(),
                text: String::new(),
                blocks: 0,
            });
    }
    #[expect(
        clippy::expect_used,
        reason = "assistant initializes the text aggregate before passing verified native \
            text to assistant_item."
    )]
    fn append_assistant_text(&mut self, text: &str) -> Result<()> {
        let aggregate = self
            .turn
            .assistant_text
            .as_mut()
            .expect("active assistant message");
        let separator = usize::from(aggregate.blocks != 0);
        if aggregate
            .text
            .len()
            .saturating_add(text.len())
            .saturating_add(separator)
            > ASSISTANT_TEXT_LIMIT
        {
            return Err(Error::output_limit(
                "Claude assistant message exceeded four MiB.",
            ));
        }
        if separator != 0 {
            aggregate.text.push('\n');
        }
        aggregate.text.push_str(text);
        aggregate.blocks += 1;
        Ok(())
    }
    pub(super) fn flush_assistant_text(&mut self) {
        if let Some(text) = self.turn.assistant_text.take()
            && text.blocks > 1
        {
            // Publish once per logical message, not after every fragment. Raw
            // items above remain in native order and provide partial crash data.
            self.notice(
                ServerMethod::ItemCompleted,
                api::value(emitted::ItemParams {
                    thread_id: &self.config.id,
                    turn_id: &text.turn,
                    item: emitted::Summary {
                        kind: "agentMessage",
                        text: text.text,
                        verified_model: true,
                        aggregation: "native_message",
                        native_message_id: text.message,
                    },
                }),
            );
        }
    }
    pub(super) fn stream(
        &mut self,
        session: &str,
        parent: Option<String>,
        uuid: Option<String>,
        event: wire::StreamEvent,
    ) -> Result<()> {
        self.check_session(session)?;
        // A forked skill's streamed text is not the agent's.
        if self.in_fork(parent.as_deref()) && self.turn.completion.is_none() {
            return Ok(());
        }
        let turn = self.turn.active.as_ref().ok_or_else(protocol)?;
        if parent.is_some()
            || self.turn.completion.is_some()
            || uuid.is_some_and(|uuid| uuid != turn.id)
        {
            return Err(protocol());
        }
        let turn = turn.id.clone();
        match event {
            wire::StreamEvent::MessageStart { message } => {
                self.check_model(&message.model)?;
                self.stream_message = message.id;
            }
            wire::StreamEvent::ContentBlockDelta { delta: Some(delta) }
                if self.live.as_ref().is_some_and(|wake| wake.live.watched()) =>
            {
                let (kind, text) = match delta {
                    wire::StreamDelta::TextDelta { text } => {
                        (crate::runtime::live::Kind::Text, text)
                    }
                    wire::StreamDelta::ThinkingDelta { thinking } => {
                        (crate::runtime::live::Kind::Reasoning, thinking)
                    }
                    wire::StreamDelta::Other => return Ok(()),
                };
                self.deltas.push(crate::runtime::provider::live_tap::Delta {
                    thread: self.config.id.clone(),
                    event: crate::runtime::live::Event {
                        turn_id: Some(turn),
                        item_id: self.stream_message.clone(),
                        text: Some(text),
                        ..crate::runtime::live::Event::new(kind)
                    },
                });
            }
            wire::StreamEvent::ContentBlockDelta { .. } | wire::StreamEvent::Other => {}
        }
        // Partial messages expose native input replay earlier, but are never
        // themselves an ACK, tool invocation, continuation or delivery receipt.
        // Only watched agents' text reaches the live hub; no frame reaches the
        // shard/worker queues.
        Ok(())
    }
}
