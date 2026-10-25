//! Transcript model: durable events and live items become display entries.
//!
//! Durable events are the record; live items show the active turn until their
//! durable counterparts arrive. The model is pure, so reconciliation is tested
//! from recorded sequences without a terminal or a daemon.
mod events;
mod live;

use crate::cli::views::{self, Event, LiveItem, Page, Payload, ProviderItem, Snapshot};
use serde_json::Value;
use std::collections::VecDeque;

/// Transcript entries kept in memory; older ones are dropped and re-paged.
pub(super) const ENTRY_LIMIT: usize = 2000;
/// Approximate transcript text kept in memory.
pub(super) const BYTE_LIMIT: usize = 8 * 1024 * 1024;
/// Streamed text or tool output kept per live entry: its newest part.
pub(super) const LIVE_TAIL: usize = 64 * 1024;
/// Live entries kept for the active turns; the oldest finished ones go first.
pub(super) const LIVE_LIMIT: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Entry {
    /// Input accepted for the agent.
    Input {
        sender: String,
        body: String,
        mode: Option<String>,
    },
    /// Assistant Markdown. `live` holds streamed text that is longer than the
    /// stored text so far (a Claude message whose later blocks are not stored).
    Assistant {
        key: Option<String>,
        text: String,
        live: String,
        aggregate: bool,
    },
    Reasoning {
        key: Option<String>,
        text: String,
    },
    Tool {
        key: Option<String>,
        tool: String,
        summary: String,
        status: Option<String>,
        output: String,
        running: bool,
    },
    TurnStarted {
        model: String,
        effort: String,
    },
    TurnFinished {
        status: String,
    },
    Approval {
        id: String,
        title: Option<String>,
        open: bool,
    },
    Notice(String),
}

impl Entry {
    fn bytes(&self) -> usize {
        match self {
            Self::Input { body, .. } => body.len(),
            Self::Assistant { text, live, .. } => text.len() + live.len(),
            Self::Reasoning { text, .. } | Self::Notice(text) => text.len(),
            Self::Tool {
                summary, output, ..
            } => summary.len() + output.len(),
            Self::TurnStarted { .. } | Self::TurnFinished { .. } | Self::Approval { .. } => 0,
        }
        .saturating_add(64)
    }
    fn key(&self) -> Option<&str> {
        match self {
            Self::Assistant { key, .. } | Self::Reasoning { key, .. } | Self::Tool { key, .. } => {
                key.as_deref()
            }
            _ => None,
        }
    }
}

/// The agent header, refreshed from every attach reply.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Header {
    pub id: String,
    /// The display name, shown before the ID.
    pub name: Option<String>,
    pub role: Option<String>,
    pub state: String,
    pub outcome: String,
    pub model: String,
    pub effort: String,
    pub sandbox: String,
    pub auth: String,
    pub auth_generation: Option<String>,
    pub auth_decision: Option<String>,
    pub waiting: Option<String>,
    pub parent: Option<String>,
    pub approval: Option<String>,
    /// The provider name, such as `codex`, once the snapshot names it.
    pub provider: Option<String>,
    pub claude: bool,
    pub steering: bool,
    pub ended: bool,
    pub queued: u64,
    pub mail: u64,
    pub children: u64,
}

impl Header {
    fn from_reply(reply: &Value) -> Self {
        let reply: crate::cli::views::Attachment<'_> = crate::cli::views::read(reply);
        let agent = reply.agent;
        Self {
            id: agent.id.unwrap_or_default().to_owned(),
            name: agent.name.map(str::to_owned),
            role: agent.role.map(str::to_owned),
            state: agent.state.unwrap_or_default().to_owned(),
            outcome: agent.task_outcome.unwrap_or_default().to_owned(),
            model: agent.model.unwrap_or_default().to_owned(),
            effort: agent.effort.unwrap_or_default().to_owned(),
            sandbox: agent.sandbox.unwrap_or_default().to_owned(),
            auth: agent.auth.unwrap_or_default().to_owned(),
            auth_generation: agent.auth_generation.map(str::to_owned),
            auth_decision: agent.auth_decision.reason.map(str::to_owned),
            waiting: agent.waiting_on.or(agent.waiting_reason).map(str::to_owned),
            parent: agent.parent_id.map(str::to_owned),
            approval: agent.approval_id.map(str::to_owned),
            provider: agent.profile.provider.map(str::to_owned),
            claude: agent.profile.provider == Some("claude"),
            steering: agent.steering_supported,
            ended: agent.ended,
            queued: reply.queued,
            mail: reply.mail,
            children: reply.children,
        }
    }
    /// A turn is running, so Enter steers (or queues) instead of starting one.
    pub(super) fn running(&self) -> bool {
        self.state == "running"
    }
}

#[derive(Debug)]
struct Item {
    seq: Option<u64>,
    entry: Entry,
}

/// A live entry and the turn it streams in.
#[derive(Debug)]
struct Live {
    turn: Option<String>,
    entry: Entry,
}

#[derive(Debug, Default)]
pub(super) struct Model {
    pub header: Header,
    items: VecDeque<Item>,
    live: Vec<Live>,
    bytes: usize,
    /// Durable forward cursor: every event up to it is applied.
    cursor: u64,
    /// Backward cursor for older history; `None` once the start is loaded.
    older: Option<u64>,
    expired: bool,
    /// A live turn is streaming.
    pub streaming: bool,
}

impl Model {
    /// Start from an attach reply with the newest page.
    pub(super) fn from_snapshot(reply: &Value) -> Self {
        let mut model = Self {
            header: Header::from_reply(reply),
            ..Self::default()
        };
        let snapshot: Snapshot<'_> = views::read(reply);
        let page = snapshot.events;
        for event in &page.events {
            model.apply_event(event);
        }
        model.cursor = page.cursor.unwrap_or(model.cursor);
        // The newest page starts at its first row; older rows may precede it.
        model.older = model.items.front().and_then(|item| item.seq);
        model.expired = page.history_expired;
        model
    }
    #[cfg(test)]
    pub(super) fn cursor(&self) -> u64 {
        self.cursor
    }
    /// The cursor for the next older page, unless history is fully loaded or the
    /// transcript is at its memory limit.
    pub(super) fn older(&self) -> Option<u64> {
        self.older
            .filter(|_| self.items.len() < ENTRY_LIMIT && self.bytes < BYTE_LIMIT)
    }
    pub(super) fn history_expired(&self) -> bool {
        self.expired && self.older.is_none()
    }
    /// Older history exists, but the transcript is at its memory limit.
    pub(super) fn full(&self) -> bool {
        self.older.is_some() && (self.items.len() >= ENTRY_LIMIT || self.bytes >= BYTE_LIMIT)
    }
    /// Refresh the header from an attach reply and apply its forward page.
    pub(super) fn apply_reply(&mut self, reply: &Value) {
        self.header = Header::from_reply(reply);
        let snapshot: Snapshot<'_> = views::read(reply);
        self.apply_events(&snapshot.events);
    }
    /// Apply a forward page; rows at or before the cursor were already applied.
    #[cfg(test)]
    pub(super) fn apply_page(&mut self, page: &Value) {
        self.apply_events(&views::read(page));
    }
    fn apply_events(&mut self, page: &Page<'_>) {
        for event in &page.events {
            if event.seq.is_some_and(|seq| seq > self.cursor) {
                self.apply_event(event);
            }
        }
        if let Some(cursor) = page.cursor {
            self.cursor = self.cursor.max(cursor);
        }
    }
    /// Prepend a history page from `attach {before}`.
    pub(super) fn prepend(&mut self, page: &Value) {
        let page: Page<'_> = views::read(page);
        let mut older = Self {
            header: self.header.clone(),
            ..Self::default()
        };
        for event in &page.events {
            older.apply_event(event);
        }
        for item in older.items.into_iter().rev() {
            self.bytes += item.entry.bytes();
            self.items.push_front(item);
        }
        self.older = page.older;
        self.expired = page.history_expired;
    }
    /// Live items were lost: drop the active turn's live area.
    pub(super) fn resync(&mut self) {
        self.live.clear();
        self.streaming = false;
        for item in &mut self.items {
            if let Entry::Assistant { live, .. } = &mut item.entry {
                live.clear();
            }
        }
    }
    /// Durable entries, oldest first, then the live area.
    pub(super) fn entries(&self) -> impl DoubleEndedIterator<Item = &Entry> {
        self.items
            .iter()
            .map(|item| &item.entry)
            .chain(self.live.iter().map(|live| &live.entry))
    }
    /// A marker for a new direct child, from the children check.
    pub(super) fn child_spawned(&mut self, id: &str, name: Option<&str>, role: Option<&str>) {
        let name = name.map_or_else(String::new, |name| format!(" \"{name}\""));
        let role = role.map_or_else(String::new, |role| format!(" ({role})"));
        self.push(
            None,
            Entry::Notice(format!("spawned child {id}{name}{role}")),
        );
    }

    /// A local notice, such as a `/usage` report.
    pub(super) fn note(&mut self, text: String) {
        self.push(None, Entry::Notice(text));
    }

    fn push(&mut self, seq: Option<u64>, entry: Entry) {
        self.bytes += entry.bytes();
        self.items.push_back(Item { seq, entry });
        while self.items.len() > ENTRY_LIMIT || self.bytes > BYTE_LIMIT {
            let Some(dropped) = self.items.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(dropped.entry.bytes());
            // Older history is paged in again from the first kept row.
            if let Some(seq) = self.items.iter().find_map(|item| item.seq) {
                self.older = Some(seq);
            }
        }
    }
}

fn kind_of<'a>(event: &Event<'a>) -> &'a str {
    event.kind.unwrap_or_default()
}

/// Missing artifact payloads remain visible without consuming readable live output.
fn artifact_notice(event: &Event<'_>) -> Option<String> {
    let kind = kind_of(event);
    if !matches!(
        kind,
        "spawned"
            | "message_accepted"
            | "redirect_accepted"
            | "assistant_message"
            | "tool_completed"
    ) {
        return None;
    }
    let id = event.data.artifact.id?;
    Some(format!(
        "{kind} content is in artifact:{id}; read it with hivectl output {id}"
    ))
}

fn sender(value: &Value) -> String {
    match value {
        Value::String(sender) => sender.clone(),
        Value::Object(fields) => fields
            .get("agent")
            .or_else(|| fields.get("id"))
            .and_then(Value::as_str)
            .unwrap_or("agent")
            .to_owned(),
        _ => String::from("unknown"),
    }
}

/// A one-line summary for notable lifecycle events; routine provider events
/// render nothing.
fn notice(kind: &str, data: &Payload<'_>) -> Option<String> {
    Some(match kind {
        "steering_submitted" => String::from("steering submitted"),
        "steering_rejected" => format!("steering rejected: {}", data.error.unwrap_or_default()),
        "steering_uncertain" => String::from("steering delivery uncertain"),
        "continuation_queued" => String::from("continuation queued"),
        "auth_switch_requested" => format!(
            "login switch to {} requested",
            data.auth.unwrap_or_default()
        ),
        "auth_switched" => format!(
            "login switched from {} to {}",
            data.from.unwrap_or_default(),
            data.to.unwrap_or_default()
        ),
        "model_rerouted" => String::from("model rerouted by the provider"),
        "options_changed" => String::from("options changed"),
        "completion_requested" => String::from("completion requested"),
        "task_completed" => String::from("task completed"),
        "termination_requested" => String::from("termination requested"),
        "lifetime_expired" => String::from("lifetime expired"),
        "execution_stopped" => String::from("stopped"),
        "execution_failed" => String::from("execution failed"),
        "branch_requested" | "branch_ready" | "branch_failed" => kind.replace('_', " "),
        _ => return None,
    })
}

fn describe_tool(item: &ProviderItem<'_>) -> (String, String) {
    match item.kind.unwrap_or_default() {
        "commandExecution" => (
            String::from("command"),
            match &item.command {
                Value::Array(words) => words
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" "),
                command => command.as_str().unwrap_or_default().to_owned(),
            },
        ),
        "fileChange" => (
            String::from("files"),
            item.changes
                .iter()
                .filter_map(|change| change.path)
                .collect::<Vec<_>>()
                .join(", "),
        ),
        "mcpToolCall" => (
            format!(
                "{}/{}",
                item.server.unwrap_or_default().to_owned(),
                item.tool.unwrap_or_default().to_owned()
            ),
            String::new(),
        ),
        _ => (item.tool.unwrap_or_default().to_owned(), String::new()),
    }
}

/// The newest `limit` bytes of `text`, starting on a character boundary.
pub(super) fn tail(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        return text;
    }
    let mut start = text.len() - limit;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

fn append_tail(target: &mut String, text: &str) {
    target.push_str(text);
    if target.len() > LIVE_TAIL {
        let kept = tail(target, LIVE_TAIL).to_owned();
        *target = kept;
    }
}

#[cfg(test)]
mod tests;
