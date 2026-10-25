//! Live, never-persisted agent items for human viewers.
//!
//! Provider shards publish streamed text, tool progress and turn markers here
//! without touching the writer. Only `attach` opens a per-agent channel, so an
//! unwatched daemon pays one atomic load per published item. Each channel keeps
//! a bounded ring; viewers long-poll with a cursor and reload durable history
//! whenever a reply asks them to resync, so a lost live item is cosmetic.
use crate::error::{Error, Result};
use crate::runtime::locks::MutexExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Semaphore, SemaphorePermit, watch};

// A ring keeps at most this many items and bytes; older items drop first.
const RING_ITEMS: usize = 512;
const RING_BYTES: usize = 256 * 1024;
// Text in one published item is truncated at this many bytes.
const ITEM_TEXT_BYTES: usize = 4 * 1024;
// Approximate per-item overhead counted against the ring byte budget.
const ITEM_OVERHEAD_BYTES: usize = 64;
// At most this many agents are watched at once.
const CHANNEL_LIMIT: usize = 256;
// Long-polls admitted at once; the rest of the control slots stay one-shot.
pub(crate) const LONG_POLL_LIMIT: usize = 32;
// A channel lives this long after its last reply; at least twice the longest wait.
const LEASE: Duration = Duration::from_secs(60);
// The longest single long-poll wait, under the client's read timeout.
pub(crate) const MAX_WAIT: Duration = Duration::from_secs(25);
// A woken tree long-poll waits this long to coalesce bursts of commits.
const TREE_DEBOUNCE: Duration = Duration::from_millis(250);
// The most items one reply carries.
pub(crate) const MAX_PAGE_ITEMS: usize = 512;
// Epochs stay within the exactly representable JSON integer range.
const EPOCH_MASK: u64 = (1 << 53) - 1;
const _: () = assert!(LEASE.as_millis() >= 2 * MAX_WAIT.as_millis());

crate::runtime::domain::domain_enum!(Kind {
    TurnStarted => "turn_started",
    Text => "text",
    Reasoning => "reasoning",
    ToolStarted => "tool_started",
    ToolOutput => "tool_output",
    ToolCompleted => "tool_completed",
    TurnCompleted => "turn_completed",
});
impl Kind {
    // Consecutive deltas of one provider item merge within a reply.
    fn mergeable(self) -> bool {
        matches!(self, Self::Text | Self::Reasoning | Self::ToolOutput)
    }
}

/// One published observation, before the hub assigns its sequence.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Event {
    pub kind: Option<Kind>,
    pub turn_id: Option<String>,
    pub item_id: Option<String>,
    pub tool: Option<String>,
    pub text: Option<String>,
    pub status: Option<String>,
}
impl Event {
    pub(crate) fn new(kind: Kind) -> Self {
        Self {
            kind: Some(kind),
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[expect(
    clippy::struct_field_names,
    reason = "`item_id` is the wire key shared with durable assistant_message events."
)]
pub(crate) struct Item {
    pub seq: u64,
    /// Milliseconds since the daemon started; ordering only, not wall time.
    pub at_ms: u64,
    pub kind: Kind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}
impl Item {
    fn bytes(&self) -> usize {
        [
            &self.turn_id,
            &self.item_id,
            &self.tool,
            &self.text,
            &self.status,
        ]
        .into_iter()
        .flatten()
        .map(String::len)
        .sum::<usize>()
            + ITEM_OVERHEAD_BYTES
    }
}

/// Resume point for one viewer of one channel.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Cursor {
    pub epoch: u64,
    pub generation: u64,
    pub seq: u64,
    pub durable: u64,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct Page {
    /// Present unless the channel is gone; then the viewer attaches again.
    pub cursor: Option<Cursor>,
    pub items: Vec<Item>,
    pub resync: bool,
    pub durable_changed: bool,
}
impl Page {
    fn resync(cursor: Option<Cursor>) -> Self {
        Self {
            cursor,
            items: Vec::new(),
            resync: true,
            durable_changed: false,
        }
    }
}

/// Version of the agent tree as seen by `tree` long-polls.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TreeVersion {
    pub epoch: u64,
    pub revision: u64,
}

struct Channel {
    generation: u64,
    ring: VecDeque<Item>,
    bytes: usize,
    next: u64,
    durable: u64,
    lease_until: Instant,
    signal: watch::Sender<(u64, u64)>,
}
impl Channel {
    fn cursor(&self, epoch: u64) -> Cursor {
        Cursor {
            epoch,
            generation: self.generation,
            seq: self.next,
            durable: self.durable,
        }
    }
    fn notify(&self) {
        self.signal.send_replace((self.next, self.durable));
    }
    fn push(&mut self, item: Item) {
        let size = item.bytes();
        while !self.ring.is_empty()
            && (self.ring.len() >= RING_ITEMS || self.bytes + size > RING_BYTES)
        {
            if let Some(old) = self.ring.pop_front() {
                self.bytes -= old.bytes();
            }
        }
        self.bytes += size;
        self.next += 1;
        self.ring.push_back(item);
        self.notify();
    }
    /// A ready page, or `None` when the viewer is current and should wait.
    fn page(&self, epoch: u64, after: Cursor, limit: usize) -> Option<Page> {
        let front = self.ring.front().map_or(self.next, |item| item.seq);
        if after.seq > self.next || after.seq < front {
            return Some(Page::resync(Some(self.cursor(epoch))));
        }
        let items: Vec<Item> = self
            .ring
            .iter()
            .skip_while(|item| item.seq < after.seq)
            .take(limit)
            .cloned()
            .collect();
        let durable_changed = after.durable != self.durable;
        if items.is_empty() && !durable_changed {
            return None;
        }
        let seq = items.last().map_or(after.seq, |item| item.seq + 1);
        Some(Page {
            cursor: Some(Cursor {
                seq,
                ..self.cursor(epoch)
            }),
            items: merge(items),
            resync: false,
            durable_changed,
        })
    }
}

/// Merge consecutive deltas of one provider item, keeping the last sequence.
fn merge(items: Vec<Item>) -> Vec<Item> {
    let mut merged: Vec<Item> = Vec::with_capacity(items.len());
    for item in items {
        if let Some(last) = merged.last_mut()
            && last.kind == item.kind
            && item.kind.mergeable()
            && last.item_id.is_some()
            && last.item_id == item.item_id
            && last.turn_id == item.turn_id
        {
            last.seq = item.seq;
            last.at_ms = item.at_ms;
            last.truncated |= item.truncated;
            if let Some(text) = item.text {
                last.text.get_or_insert_with(String::new).push_str(&text);
            }
            continue;
        }
        merged.push(item);
    }
    merged
}

fn capped(text: String) -> (String, bool) {
    if text.len() <= ITEM_TEXT_BYTES {
        return (text, false);
    }
    let mut end = ITEM_TEXT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_owned(), true)
}

pub struct LiveHub {
    epoch: u64,
    started: Instant,
    viewers: AtomicUsize,
    generations: AtomicU64,
    channels: Mutex<HashMap<String, Channel>>,
    tree: watch::Sender<u64>,
    stopping: watch::Sender<bool>,
    polls: Semaphore,
}
impl Default for LiveHub {
    fn default() -> Self {
        let (low, _) = uuid::Uuid::new_v4().as_u64_pair();
        Self {
            epoch: low & EPOCH_MASK,
            started: Instant::now(),
            viewers: AtomicUsize::new(0),
            generations: AtomicU64::new(0),
            channels: Mutex::default(),
            tree: watch::Sender::new(0),
            stopping: watch::Sender::new(false),
            polls: Semaphore::new(LONG_POLL_LIMIT),
        }
    }
}
impl LiveHub {
    #[cfg(test)]
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }
    /// True while any channel exists; publishers return early otherwise.
    pub(crate) fn watched(&self) -> bool {
        self.viewers.load(Ordering::Acquire) > 0
    }
    /// Publish one observation for a watched agent; otherwise build nothing.
    pub(crate) fn publish(&self, agent: &str, event: impl FnOnce() -> Option<Event>) {
        if !self.watched() {
            return;
        }
        let mut channels = self.channels.lock_or_panic();
        let Some(channel) = channels.get_mut(agent) else {
            return;
        };
        let Some(Event {
            kind: Some(kind),
            turn_id,
            item_id,
            tool,
            text,
            status,
        }) = event()
        else {
            return;
        };
        let (text, truncated) = match text.map(capped) {
            Some((text, truncated)) => (Some(text), truncated),
            None => (None, false),
        };
        let at_ms = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        channel.push(Item {
            seq: channel.next,
            at_ms,
            kind,
            turn_id,
            item_id,
            tool,
            text,
            status,
            truncated,
        });
    }
    /// Items were lost before publication; current viewers of this agent resync.
    pub(crate) fn gap(&self, agent: &str) {
        if !self.watched() {
            return;
        }
        if let Some(channel) = self.channels.lock_or_panic().get_mut(agent) {
            channel.ring.clear();
            channel.bytes = 0;
            channel.next += 1;
            channel.notify();
        }
    }
    /// Durable history changed for these agents (or all, after overflow).
    pub(crate) fn durable_changed<'a>(&self, agents: impl IntoIterator<Item = &'a str>, all: bool) {
        if !self.watched() {
            return;
        }
        let mut channels = self.channels.lock_or_panic();
        self.sweep(&mut channels, Instant::now());
        let mut bump = |channel: &mut Channel| {
            channel.durable += 1;
            channel.notify();
        };
        if all {
            channels.values_mut().for_each(&mut bump);
        } else {
            for agent in agents {
                if let Some(channel) = channels.get_mut(agent) {
                    bump(channel);
                }
            }
        }
    }
    /// Agents changed in a committed batch; wakes `tree` long-polls.
    pub(crate) fn tree_changed(&self) {
        self.tree.send_modify(|revision| *revision += 1);
    }
    pub(crate) fn tree_version(&self) -> TreeVersion {
        TreeVersion {
            epoch: self.epoch,
            revision: *self.tree.borrow(),
        }
    }
    /// Open or renew the channel for an agent; `true` when newly created.
    pub(crate) fn open(&self, agent: &str) -> Result<(Cursor, bool)> {
        let now = Instant::now();
        let mut channels = self.channels.lock_or_panic();
        self.sweep(&mut channels, now);
        if let Some(channel) = channels.get_mut(agent) {
            channel.lease_until = now + LEASE;
            return Ok((channel.cursor(self.epoch), false));
        }
        if channels.len() >= CHANNEL_LIMIT {
            return Err(Error::watch_capacity(
                "Too many agents are being watched; close a viewer and retry.",
            ));
        }
        let channel = Channel {
            generation: self.generations.fetch_add(1, Ordering::Relaxed) + 1,
            ring: VecDeque::new(),
            bytes: 0,
            next: 0,
            durable: 0,
            lease_until: now + LEASE,
            signal: watch::Sender::new((0, 0)),
        };
        let cursor = channel.cursor(self.epoch);
        // Count the viewer first: a publisher that sees it then waits for this
        // lock and finds the channel, so nothing after the cursor is skipped.
        self.viewers.fetch_add(1, Ordering::Release);
        channels.insert(agent.to_owned(), channel);
        Ok((cursor, true))
    }
    /// Remove a channel opened for an agent that did not resolve.
    pub(crate) fn discard(&self, agent: &str, generation: u64) {
        let mut channels = self.channels.lock_or_panic();
        if channels
            .get(agent)
            .is_some_and(|channel| channel.generation == generation)
        {
            channels.remove(agent);
            self.viewers.fetch_sub(1, Ordering::Release);
        }
    }
    fn sweep(&self, channels: &mut HashMap<String, Channel>, now: Instant) {
        let before = channels.len();
        channels.retain(|_, channel| channel.lease_until > now);
        let removed = before - channels.len();
        if removed > 0 {
            self.viewers.fetch_sub(removed, Ordering::Release);
        }
    }
    /// Admit one long-poll, or refuse it so one-shot operations keep their slots.
    pub(crate) fn admit(&self) -> Result<SemaphorePermit<'_>> {
        self.polls.try_acquire().map_err(|cause| {
            Error::watch_capacity("Too many viewers are waiting; retry after a short delay.")
                .with_source(cause)
        })
    }
    /// A long-poll permit only for a request that can wait; snapshot reads
    /// never compete for one.
    pub(crate) fn admit_if(&self, waits: bool) -> Result<Option<SemaphorePermit<'_>>> {
        waits.then(|| self.admit()).transpose()
    }
    /// Wake every long-poll with `daemon_stopping`; called once when control stops.
    pub(crate) fn stop(&self) {
        self.stopping.send_replace(true);
    }
    fn stopping_error() -> Error {
        Error::daemon_stopping("The daemon is stopping; reconnect after it restarts.")
    }
    /// Items after `after`, waiting up to `wait` when the viewer is current.
    pub(crate) async fn next(
        &self,
        agent: &str,
        after: Cursor,
        wait: Duration,
        limit: usize,
    ) -> Result<Page> {
        let mut stopping = self.stopping.subscribe();
        if *stopping.borrow() {
            return Err(Self::stopping_error());
        }
        let mut signal = {
            let mut channels = self.channels.lock_or_panic();
            self.sweep(&mut channels, Instant::now());
            let Some(channel) = channels.get_mut(agent) else {
                return Ok(Page::resync(None));
            };
            if after.epoch != self.epoch || after.generation != channel.generation {
                return Ok(Page::resync(Some(channel.cursor(self.epoch))));
            }
            if let Some(page) = channel.page(self.epoch, after, limit) {
                channel.lease_until = Instant::now() + LEASE;
                return Ok(page);
            }
            channel.signal.subscribe()
        };
        tokio::select! {
            _ = signal.changed() => {}
            () = tokio::time::sleep(wait) => {}
            _ = stopping.wait_for(|stopping| *stopping) => return Err(Self::stopping_error()),
        }
        let mut channels = self.channels.lock_or_panic();
        let Some(channel) = channels.get_mut(agent) else {
            return Ok(Page::resync(None));
        };
        if after.generation != channel.generation {
            return Ok(Page::resync(Some(channel.cursor(self.epoch))));
        }
        channel.lease_until = Instant::now() + LEASE;
        Ok(channel
            .page(self.epoch, after, limit)
            .unwrap_or_else(|| Page {
                cursor: Some(after),
                items: Vec::new(),
                resync: false,
                durable_changed: false,
            }))
    }
    /// Wait until the tree revision moves past `after`, or return at once.
    pub(crate) async fn tree_after(
        &self,
        after: Option<TreeVersion>,
        wait: Duration,
    ) -> Result<TreeVersion> {
        let mut stopping = self.stopping.subscribe();
        if *stopping.borrow() {
            return Err(Self::stopping_error());
        }
        let mut revision = self.tree.subscribe();
        let current = *revision.borrow_and_update();
        if after.is_none_or(|after| after.epoch != self.epoch || after.revision != current) {
            return Ok(self.tree_version());
        }
        let changed = tokio::select! {
            changed = revision.changed() => changed.is_ok(),
            () = tokio::time::sleep(wait) => false,
            _ = stopping.wait_for(|stopping| *stopping) => return Err(Self::stopping_error()),
        };
        if changed {
            // Coalesce bursts: at most four replies a second per viewer.
            tokio::select! {
                () = tokio::time::sleep(TREE_DEBOUNCE) => {}
                _ = stopping.wait_for(|stopping| *stopping) => return Err(Self::stopping_error()),
            }
        }
        Ok(self.tree_version())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Fixtures stop immediately on setup failure.
mod tests;
