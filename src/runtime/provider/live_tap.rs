//! Map provider frames to live items for watched agents only.
//!
//! Delta notifications are taken from the raw frame before lifecycle observation;
//! the shard attributes them by native thread. Lifecycle frames are copied when
//! the shard selects their owner, before routing, so a failing or draining
//! owner's viewer still sees them. Only an explicit set of item kinds is shown:
//! user-message echoes, hook prompts and unknown items never become live items.
use super::protocol::{IncomingFrame, ServerMethod};
use crate::runtime::live::{Event, Kind};
use serde_json::Value;
use std::collections::VecDeque;

// Native method names of the streamed delta families Hive displays.
const AGENT_MESSAGE_DELTA: &str = "item/agentMessage/delta";
const COMMAND_OUTPUT_DELTA: &str = "item/commandExecution/outputDelta";
const REASONING_SUMMARY_DELTA: &str = "item/reasoning/summaryTextDelta";
// Raw reasoning stays opted out; a provider that sends it anyway is ignored here.
const RAW_REASONING_DELTA: &str = "item/reasoning/textDelta";
// Deltas held between shard passes; the oldest drop first when viewers fall behind.
const QUEUED_DELTAS: usize = 1024;
// Threads remembered as having lost deltas before every viewer is resynced.
const QUEUED_GAPS: usize = 64;

/// One streamed delta, attributed to an agent by its native thread.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Delta {
    pub thread: String,
    pub event: Event,
}

/// Bounded deltas between the provider reader and the shard's publication.
///
/// Each delta keeps its provider read sequence, so the shard publishes it just
/// before the first later frame even when that frame was deferred during a call.
/// Overflow drops the oldest delta and records its thread, whose viewers then
/// resync instead of silently missing text.
#[derive(Default)]
pub(crate) struct Deltas {
    queue: VecDeque<(u64, Delta)>,
    gaps: std::collections::HashSet<String>,
    all_gaps: bool,
}

/// Deltas ready for publication, after any threads that lost deltas.
pub(crate) struct Ready {
    pub deltas: Vec<Delta>,
    pub gaps: Vec<String>,
    pub all_gaps: bool,
}

impl Deltas {
    pub(crate) fn push(&mut self, seq: u64, delta: Delta) {
        if self.queue.len() >= QUEUED_DELTAS
            && let Some((_, lost)) = self.queue.pop_front()
        {
            if self.gaps.len() < QUEUED_GAPS {
                self.gaps.insert(lost.thread);
            } else {
                self.all_gaps = true;
            }
        }
        self.queue.push_back((seq, delta));
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.queue.is_empty() && self.gaps.is_empty() && !self.all_gaps
    }
    /// Deltas read before `seq`, or all of them when no frame follows.
    pub(crate) fn before(&mut self, seq: Option<u64>) -> Ready {
        let count = seq.map_or(self.queue.len(), |seq| {
            self.queue
                .iter()
                .take_while(|(read, _)| *read < seq)
                .count()
        });
        Ready {
            deltas: self.queue.drain(..count).map(|(_, delta)| delta).collect(),
            gaps: self.gaps.drain().collect(),
            all_gaps: std::mem::take(&mut self.all_gaps),
        }
    }
}

/// A delta notification: shown to viewers, or hidden raw reasoning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeltaKind {
    Shown(Kind),
    Hidden,
}

/// The delta kind of a raw notification; `None` for every other frame.
pub(crate) fn delta_kind(frame: &Value) -> Option<DeltaKind> {
    match frame.get("method")?.as_str()? {
        AGENT_MESSAGE_DELTA => Some(DeltaKind::Shown(Kind::Text)),
        COMMAND_OUTPUT_DELTA => Some(DeltaKind::Shown(Kind::ToolOutput)),
        REASONING_SUMMARY_DELTA => Some(DeltaKind::Shown(Kind::Reasoning)),
        RAW_REASONING_DELTA => Some(DeltaKind::Hidden),
        _ => None,
    }
}

fn string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// A delta from a raw notification; malformed deltas are ignored, never fatal.
pub(crate) fn delta(kind: Kind, frame: &Value) -> Option<Delta> {
    let params = frame.get("params")?;
    Some(Delta {
        thread: string(params, "threadId")?,
        event: Event {
            turn_id: string(params, "turnId"),
            item_id: string(params, "itemId"),
            text: Some(string(params, "delta")?),
            ..Event::new(kind)
        },
    })
}

/// The tool label and start summary for an allowlisted tool item.
fn tool(item: &Value) -> Option<(String, Option<String>)> {
    Some(match item.get("type")?.as_str()? {
        "commandExecution" => (
            "command".into(),
            match item.get("command") {
                Some(Value::String(command)) => Some(command.clone()),
                Some(Value::Array(parts)) => Some(
                    parts
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                _ => None,
            },
        ),
        "fileChange" => (
            "file_change".into(),
            item.get("changes")
                .and_then(Value::as_array)
                .map(|changes| {
                    changes
                        .iter()
                        .filter_map(|change| change.get("path").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join(", ")
                }),
        ),
        "dynamicToolCall" => (string(item, "tool")?, None),
        "mcpToolCall" => (
            format!("{}/{}", string(item, "server")?, string(item, "tool")?),
            None,
        ),
        _ => return None,
    })
}

/// The live copy of an allowlisted lifecycle frame.
pub(crate) fn event(frame: &IncomingFrame) -> Option<Event> {
    let params = frame.params().raw();
    match frame.method()? {
        method @ (ServerMethod::TurnStarted | ServerMethod::TurnCompleted) => {
            let turn = params.get("turn")?;
            Some(Event {
                turn_id: string(turn, "id"),
                status: string(turn, "status"),
                ..Event::new(if *method == ServerMethod::TurnStarted {
                    Kind::TurnStarted
                } else {
                    Kind::TurnCompleted
                })
            })
        }
        method @ (ServerMethod::ItemStarted | ServerMethod::ItemCompleted) => {
            let item = params.get("item")?;
            let (tool, summary) = tool(item)?;
            let started = *method == ServerMethod::ItemStarted;
            let exit = item
                .get("exitCode")
                .and_then(Value::as_i64)
                .map(|code| format!("exit {code}"));
            Some(Event {
                turn_id: string(params, "turnId"),
                item_id: string(item, "id"),
                tool: Some(tool),
                text: if started { summary } else { exit },
                status: (!started).then(|| string(item, "status")).flatten(),
                ..Event::new(if started {
                    Kind::ToolStarted
                } else {
                    Kind::ToolCompleted
                })
            })
        }
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Fixtures stop immediately on malformed literals.
mod tests;
