//! Follow the agent tree on a detached thread, one long-poll at a time, for
//! `hivectl panes` and the `hivectl tui` sidebar.
use super::observe::{self, Observation};
use crate::error::ErrorCode;
use crate::runtime;
use serde_json::Value;
use std::{path::PathBuf, sync::mpsc::SyncSender, time::Duration};

const LONG_POLL_MS: u64 = 25_000;
const MAX_BACKOFF: Duration = Duration::from_secs(5);

pub(super) enum Feed {
    /// A tree reply, with `agents` and `truncated`.
    Tree(Value),
    /// The daemon has no room for another viewer; the follower keeps retrying.
    Waiting,
    Disconnected,
    /// The first read was refused (for example, an unknown root).
    Refused(Value),
}

/// `scope` is a root agent, or every live agent when `None`.
pub(super) fn follow(root: PathBuf, scope: Option<String>, feed: SyncSender<Feed>) {
    let mut version = Value::Null;
    let mut attempt = 0_u32;
    let mut first = true;
    loop {
        let following = !version.is_null();
        let request = observe::value(Observation::Tree {
            root: scope.as_deref(),
            all: scope.is_none().then_some(true),
            after: following.then_some(&version),
            wait_ms: following.then_some(LONG_POLL_MS),
        });
        let update = match runtime::client(&root, &uuid::Uuid::new_v4().to_string(), &request) {
            Ok(reply) if reply.get("error").is_none() => {
                (attempt, first) = (0, false);
                version = super::views::read::<super::views::TreeMetadata>(&reply).version;
                Feed::Tree(reply)
            }
            Ok(reply) if super::views::error_code(&reply) == ErrorCode::WatchCapacity => {
                std::thread::sleep(IDLE_REFRESH_DELAY);
                Feed::Waiting
            }
            // A refusal of the first read is final; a stopping daemon is not
            // a refusal, and is waited for like an unreachable one.
            Ok(reply) if first && super::views::error_code(&reply) != ErrorCode::DaemonStopping => {
                let _ = feed.send(Feed::Refused(reply));
                return;
            }
            Ok(_) | Err(_) => {
                version = Value::Null;
                std::thread::sleep(Duration::from_millis(250 << attempt.min(4)).min(MAX_BACKOFF));
                attempt += 1;
                Feed::Disconnected
            }
        };
        if feed.send(update).is_err() {
            return;
        }
    }
}

const IDLE_REFRESH_DELAY: std::time::Duration = Duration::from_secs(1);
