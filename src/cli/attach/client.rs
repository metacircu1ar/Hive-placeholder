//! Daemon traffic off the UI thread: a live follower and a command worker.
//!
//! Dropping the view stops further reads after an outstanding poll returns.
//! Accepted writes drain independently, retaining their stable retry IDs.
mod commands;
#[cfg(test)]
use commands::retry;
use commands::{deliver, problem, work};

mod artifacts;
use crate::cli::observe::{self, Observation};
use crate::cli::views::{self, AttachMetadata, Detail, Listing, LivePage, RawAttachment};
use crate::error::ErrorCode;
use crate::runtime;
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender, SyncSender},
    },
    time::Duration,
};

/// Updates queued for the UI before the follower waits for it to catch up.
pub(super) const UPDATE_SLOTS: usize = 256;
const LONG_POLL_MS: u64 = 25_000;
/// The wait after a capacity refusal: short, so the status clears soon after a
/// slot is free.
const SLOT_PROBE_MS: u64 = 1_000;
const HISTORY_ROWS: u64 = 200;
const RETRY_LIMIT: u32 = 5;
const MAX_BACKOFF: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub(super) enum Update {
    /// The first attach reply, with the newest page.
    Snapshot(Value),
    /// An `attach {after}` reply: the header and the next forward page.
    Refresh(Value),
    Live(Vec<Value>),
    Resync,
    Disconnected,
    /// The agent no longer exists.
    Purged,
    /// The pending approval's details, or `Null` once none is pending.
    Approval(Value),
    /// Direct children that appeared since the view opened.
    Children(Vec<Value>),
    /// An `attach {before}` page, or why it could not be read.
    History(Result<Value, String>),
    /// A login's usage reports, or why they could not be read.
    Usage {
        auth: String,
        result: Result<Value, String>,
    },
    /// The daemon refused a request the view keeps retrying.
    Problem(String),
    /// The daemon has no room for another viewer (`true`) or admitted this one again.
    Waiting(bool),
    /// The daemon refused the first attach; this is its error reply.
    Refused(Value),
    Done {
        action: u64,
        label: String,
        result: Result<Value, String>,
    },
}

#[derive(Debug)]
pub(super) enum Command {
    /// One user action. `id` stays the same for every retry, so the daemon
    /// replays instead of repeating the write.
    Write {
        action: u64,
        id: String,
        label: String,
        request: Value,
    },
    History {
        before: u64,
    },
    /// Read a login's reported usage.
    Usage {
        auth: String,
    },
}

enum Outcome {
    Ok(Value),
    Failed {
        code: ErrorCode,
        message: String,
        reply: Value,
    },
    Transport,
}

fn call(root: &Path, id: &str, request: &Value) -> Outcome {
    match runtime::client(root, id, request) {
        Ok(reply) => classify(reply),
        Err(_) => Outcome::Transport,
    }
}

/// A reply's outcome. A stopping daemon is about to be unreachable, so it is
/// waited for and retried like one, never shown as a refusal.
fn classify(reply: Value) -> Outcome {
    let Ok(response) = runtime::api::DaemonReply::new(&reply) else {
        return Outcome::Transport;
    };
    match response.error_code() {
        Some(ErrorCode::DaemonStopping) => Outcome::Transport,
        Some(code) => Outcome::Failed {
            code,
            message: response.error_message().unwrap_or_default().to_owned(),
            reply,
        },
        None => Outcome::Ok(reply),
    }
}

fn read(root: &Path, request: &Value) -> Outcome {
    call(root, &uuid::Uuid::new_v4().to_string(), request)
}

fn backoff(attempt: u32) -> Duration {
    Duration::from_millis(250 << attempt.min(5)).min(MAX_BACKOFF)
}

pub(super) struct Subscription(Arc<AtomicBool>);
impl Drop for Subscription {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

pub(super) fn start(
    root: PathBuf,
    target: String,
) -> (Receiver<Update>, Sender<Command>, Subscription) {
    let (updates, received) = std::sync::mpsc::sync_channel(UPDATE_SLOTS);
    let (commands, pending) = std::sync::mpsc::channel();
    let reader = root.clone();
    let follower = Follower::new(
        target.clone(),
        updates.clone(),
        Box::new(move |request| read(&reader, request)),
        std::thread::sleep,
    );
    let cancelled = Arc::clone(&follower.cancelled);
    let subscription = Subscription(Arc::clone(&cancelled));
    std::thread::spawn(move || follower.run());
    std::thread::spawn(move || {
        work(
            &root,
            &target,
            &pending,
            &updates,
            &cancelled,
            |id, request| deliver(&root, id, request),
        );
    });
    (received, commands, subscription)
}

/// One read request and its outcome; tests script these.
type Reader = Box<dyn FnMut(&Value) -> Outcome + Send>;

struct Follower {
    target: String,
    updates: SyncSender<Update>,
    reader: Reader,
    pause: fn(Duration),
    cancelled: Arc<AtomicBool>,
    /// The live cursor; `Null` until attached.
    live: Value,
    /// The durable event cursor every applied page reached.
    durable: u64,
    approval: Option<String>,
    /// Known direct children, once the first count has been seen.
    children: Option<u64>,
    /// Monotonic listing cursor, including children subsequently removed.
    child_after: Option<u64>,
    /// The last capacity state reported to the view.
    waiting: bool,
}

impl Follower {
    fn new(
        target: String,
        updates: SyncSender<Update>,
        reader: Reader,
        pause: fn(Duration),
    ) -> Self {
        Self {
            target,
            updates,
            reader,
            pause,
            cancelled: Arc::new(AtomicBool::new(false)),
            live: Value::Null,
            durable: 0,
            approval: None,
            children: None,
            child_after: None,
            waiting: false,
        }
    }

    fn send(&self, update: Update) -> bool {
        !self.cancelled.load(Ordering::Acquire) && self.updates.send(update).is_ok()
    }

    fn read(&mut self, request: &Value) -> Outcome {
        if self.cancelled.load(Ordering::Acquire) {
            return Outcome::Transport;
        }
        let mut outcome = (self.reader)(request);
        if self.cancelled.load(Ordering::Acquire) {
            return Outcome::Transport;
        }
        if request["op"] == "attach"
            && let Outcome::Ok(reply) = &mut outcome
        {
            artifacts::expand(reply, &mut self.reader, &self.cancelled);
        }
        outcome
    }

    /// Report a change in capacity; `false` once the view is gone.
    fn slot(&mut self, waiting: bool) -> bool {
        if self.waiting == waiting {
            return true;
        }
        self.waiting = waiting;
        self.send(Update::Waiting(waiting))
    }

    fn run(mut self) {
        let mut attempt = 0;
        while !self.cancelled.load(Ordering::Acquire) {
            let request = observe::value(Observation::Attach {
                target: &self.target,
                before: None,
                after: None,
                limit: None,
            });
            match self.read(&request) {
                Outcome::Ok(reply) => {
                    if !self.slot(false) {
                        return;
                    }
                    self.adopt(&reply, true);
                    let summary = Self::summary(&reply);
                    if !self.send(Update::Snapshot(reply)) || !self.after_reply(summary) {
                        return;
                    }
                    break;
                }
                // Every live channel is taken: wait for one to close.
                Outcome::Failed {
                    code: ErrorCode::WatchCapacity,
                    ..
                } => {
                    if !self.slot(true) {
                        return;
                    }
                    (self.pause)(backoff(attempt));
                    attempt += 1;
                }
                // An unknown agent or any other refusal ends the view with the
                // daemon's error; only an unreachable daemon is waited for.
                Outcome::Failed { reply, .. } => {
                    self.send(Update::Refused(reply));
                    return;
                }
                Outcome::Transport => {
                    if !self.send(Update::Disconnected) {
                        return;
                    }
                    (self.pause)(backoff(attempt));
                    attempt += 1;
                }
            }
        }
        // The newest page can end early at its byte budget: page forward to the
        // end before following, or an idle agent would show stale history.
        if self.refresh(false) {
            self.follow();
        }
    }

    fn follow(&mut self) {
        let mut attempt = 0;
        while !self.cancelled.load(Ordering::Acquire) {
            let wait = if self.waiting {
                SLOT_PROBE_MS
            } else {
                LONG_POLL_MS
            };
            let request = observe::value(Observation::Live {
                target: &self.target,
                after: &self.live,
                wait_ms: wait,
            });
            let page = match self.read(&request) {
                Outcome::Ok(page) => page,
                Outcome::Failed {
                    code: ErrorCode::WatchCapacity,
                    ..
                } => {
                    if !self.slot(true) {
                        return;
                    }
                    (self.pause)(IDLE_REFRESH_DELAY);
                    continue;
                }
                Outcome::Failed {
                    code: ErrorCode::UnknownAgent,
                    ..
                } => {
                    self.send(Update::Purged);
                    return;
                }
                failure => {
                    if !self.send(problem(failure)) {
                        return;
                    }
                    (self.pause)(backoff(attempt));
                    attempt += 1;
                    // A new live cursor follows: anything streamed meanwhile is lost.
                    if !self.send(Update::Resync) || !self.refresh(true) {
                        return;
                    }
                    continue;
                }
            };
            attempt = 0;
            if !self.slot(false) {
                return;
            }
            let page: LivePage = views::read(&page);
            if page.cursor.is_null() || page.resync {
                // The watch ended or items were lost: rebuild from durable history
                // and a new live cursor.
                if !self.send(Update::Resync) || !self.refresh(true) {
                    return;
                }
                continue;
            }
            self.live = page.cursor;
            let items = page.items;
            if !items.is_empty() && !self.send(Update::Live(items)) {
                return;
            }
            if page.durable_changed && !self.refresh(false) {
                return;
            }
        }
    }

    /// Apply forward pages until one is empty; adopt the live cursor when asked.
    fn refresh(&mut self, adopt_live: bool) -> bool {
        let mut adopt = adopt_live;
        while !self.cancelled.load(Ordering::Acquire) {
            let request = observe::value(Observation::Attach {
                target: &self.target,
                before: None,
                after: Some(self.durable),
                limit: Some(HISTORY_ROWS),
            });
            match self.read(&request) {
                Outcome::Ok(reply) => {
                    if !self.slot(false) {
                        return false;
                    }
                    let empty = views::read::<AttachMetadata<'_>>(&reply)
                        .events
                        .events
                        .is_empty();
                    self.adopt(&reply, adopt);
                    adopt = false;
                    let summary = Self::summary(&reply);
                    if !self.send(Update::Refresh(reply)) || !self.after_reply(summary) {
                        return false;
                    }
                    if empty {
                        return true;
                    }
                }
                Outcome::Failed {
                    code: ErrorCode::UnknownAgent,
                    ..
                } => {
                    self.send(Update::Purged);
                    return false;
                }
                Outcome::Failed {
                    code: ErrorCode::WatchCapacity,
                    ..
                } => {
                    if !self.slot(true) {
                        return false;
                    }
                    (self.pause)(IDLE_REFRESH_DELAY);
                }
                failure => {
                    if !self.send(problem(failure)) {
                        return false;
                    }
                    (self.pause)(MAX_BACKOFF);
                }
            }
        }
        false
    }

    fn adopt(&mut self, reply: &Value, live: bool) {
        let reply: AttachMetadata<'_> = views::read(reply);
        if let Some(cursor) = reply.events.cursor {
            self.durable = self.durable.max(cursor);
        }
        if live {
            self.live = reply.live;
        }
    }

    fn summary(reply: &Value) -> (Option<String>, u64) {
        let reply: AttachMetadata<'_> = views::read(reply);
        (reply.agent.approval_id.map(str::to_owned), reply.children)
    }

    /// Fetch a newly pending approval and newly spawned children.
    fn after_reply(&mut self, (approval, children): (Option<String>, u64)) -> bool {
        if approval != self.approval {
            let detail = match &approval {
                Some(id) => {
                    match self.read(&observe::value(Observation::Approval { approval_id: id })) {
                        Outcome::Ok(reply) => Some(views::read::<Detail>(&reply).approval),
                        // Keep the previous card; the next reply fetches it again.
                        failure => {
                            if !self.send(problem(failure)) {
                                return false;
                            }
                            None
                        }
                    }
                }
                None => Some(Value::Null),
            };
            if let Some(detail) = detail {
                self.approval = approval;
                if !self.send(Update::Approval(detail)) {
                    return false;
                }
            }
        }
        if children == 0 {
            self.children = Some(0);
            return true;
        }
        let announce = self.children.is_some();
        // Counts can stay equal when an old child is purged and a new one
        // appears. Always probe past the cursor, keeping no unbounded ID set.
        loop {
            let request = observe::value(Observation::Agents {
                parent: &self.target,
                limit: HISTORY_ROWS,
                after: self.child_after.unwrap_or(0),
            });
            let Outcome::Ok(reply) = self.read(&request) else {
                return !self.cancelled.load(Ordering::Acquire);
            };
            let reply: Listing = views::read(&reply);
            let rows = reply.agents;
            if rows.is_empty() {
                self.children = Some(children);
                return true;
            }
            let Some(next) = reply
                .next_after
                .filter(|next| *next > self.child_after.unwrap_or(0))
            else {
                return self.send(Update::Problem(String::from(
                    "child listing did not advance",
                )));
            };
            if announce && !self.send(Update::Children(rows)) {
                return false;
            }
            self.child_after = Some(next);
        }
    }
}

#[cfg(test)]
mod tests;

const IDLE_REFRESH_DELAY: std::time::Duration = Duration::from_secs(1);
