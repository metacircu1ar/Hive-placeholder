//! Commands for this component.
use super::views;
use super::{
    AtomicBool, Command, Detail, HISTORY_ROWS, Ordering, Outcome, Path, RETRY_LIMIT, RawAttachment,
    Receiver, SyncSender, Update, Value, artifacts, backoff, call, read,
};
use crate::cli::observe::{self, Observation};

pub(super) fn work(
    root: &Path,
    target: &str,
    pending: &Receiver<Command>,
    updates: &SyncSender<Update>,
    cancelled: &AtomicBool,
    mut deliver: impl FnMut(&str, &Value) -> Result<Value, String>,
) {
    while let Ok(command) = pending.recv() {
        let update = match command {
            Command::Write {
                action,
                id,
                label,
                request,
            } => Update::Done {
                action,
                label,
                result: deliver(&id, &request),
            },
            Command::History { .. } | Command::Usage { .. }
                if cancelled.load(Ordering::Acquire) =>
            {
                continue;
            }
            Command::History { before } => {
                let request = observe::value(Observation::Attach {
                    target,
                    before: Some(before),
                    after: None,
                    limit: Some(HISTORY_ROWS),
                });
                Update::History(match read(root, &request) {
                    Outcome::Ok(mut reply) => {
                        artifacts::expand(
                            &mut reply,
                            &mut |request| read(root, request),
                            cancelled,
                        );
                        Ok(views::read::<RawAttachment>(&reply).events)
                    }
                    failure => Err(describe(failure)),
                })
            }
            Command::Usage { auth } => {
                let request = observe::value(Observation::Usage { auth: &auth });
                let result = match read(root, &request) {
                    Outcome::Ok(reply) => Ok(views::read::<Detail>(&reply).usage),
                    failure => Err(describe(failure)),
                };
                Update::Usage { auth, result }
            }
        };
        // Closing the view drops its command sender. Drain previously queued
        // writes even when their receipts can no longer be shown.
        let _ = updates.send(update);
    }
}

pub(super) fn describe(outcome: Outcome) -> String {
    match outcome {
        Outcome::Failed { code, message, .. } => format!("{}: {message}", code.as_str()),
        Outcome::Ok(_) | Outcome::Transport => String::from("the daemon did not answer"),
    }
}

/// A refused request is shown; an unreachable daemon shows as disconnected.
pub(super) fn problem(outcome: Outcome) -> Update {
    match outcome {
        Outcome::Transport => Update::Disconnected,
        failure => Update::Problem(describe(failure)),
    }
}

pub(super) fn deliver(root: &Path, id: &str, request: &Value) -> Result<Value, String> {
    retry(
        id,
        |id| call(root, id, request),
        |attempt| std::thread::sleep(backoff(attempt)),
    )
}

/// Send one write, retrying transport failures with the same request ID, so a
/// write whose reply was lost is answered from the daemon's replay record.
pub(super) fn retry(
    id: &str,
    mut call: impl FnMut(&str) -> Outcome,
    mut wait: impl FnMut(u32),
) -> Result<Value, String> {
    for attempt in 0..RETRY_LIMIT {
        match call(id) {
            Outcome::Ok(reply) => return Ok(reply),
            Outcome::Failed { code, message, .. } => {
                return Err(format!("{}: {message}", code.as_str()));
            }
            Outcome::Transport => wait(attempt),
        }
    }
    Err(String::from(
        "daemon_unavailable: the daemon did not answer; retry later",
    ))
}
