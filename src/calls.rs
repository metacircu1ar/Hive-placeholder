//! Bounded external observers. No wait request or cancellation reaches the writer.
#[cfg(test)]
use crate::error::ErrorCode;
#[cfg(test)]
use crate::runtime::client::{Reply, decode};
mod error;
use crate::{
    error::{Error, Result},
    runtime::{
        api::{self, CallOutcome, MessageResult, Request, SendArgs},
        domain::DeliveryMode,
    },
};
pub(crate) use error::ObservationError;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{path::Path, time::Duration};
use tokio::time::Instant;

// A caller observes for 45 seconds unless it requests another bounded deadline.
pub(crate) const DEFAULT_TIMEOUT: u64 = 45;
// Observation never waits less than one second or longer than ten minutes.
pub(crate) const MIN_TIMEOUT: u64 = 1;
pub(crate) const MAX_TIMEOUT: u64 = 600;
// One-second polling plus stable sub-200 ms jitter spreads concurrent observers.
const POLL_MILLIS: u64 = 1000;
const JITTER_MILLIS: u64 = 200;
const _: () = assert!(MIN_TIMEOUT <= DEFAULT_TIMEOUT && DEFAULT_TIMEOUT <= MAX_TIMEOUT);
const _: () = assert!(JITTER_MILLIS < POLL_MILLIS);

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CallResult {
    pub outcome: CallOutcome,
    pub message_id: Option<String>,
    pub request_id: Option<String>,
    pub result: Option<MessageResult>,
    pub error: Option<ObservationError>,
}
impl CallResult {
    fn new(message: Option<String>, request: Option<String>) -> Self {
        Self {
            outcome: CallOutcome::Pending,
            message_id: message,
            request_id: request,
            result: None,
            error: None,
        }
    }
    fn unavailable(mut self, error: ObservationError) -> Self {
        self.outcome = CallOutcome::ObservationUnavailable;
        self.error = Some(error);
        self
    }
}

pub(crate) fn executor() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(Error::state_io_from)
}

pub(crate) async fn exchange<T: DeserializeOwned>(
    root: &Path,
    id: &str,
    args: &Request,
    deadline: Instant,
) -> std::result::Result<T, ObservationError> {
    crate::runtime::client::Client::new(root)
        .observe(id, args, deadline)
        .await
}

pub(crate) async fn send(
    root: &Path,
    id: &str,
    args: SendArgs,
    timeout: u64,
    progress: impl FnMut(&CallResult),
) -> CallResult {
    let result = CallResult::new(None, Some(id.into()));
    if !args.delay_seconds.is_missing()
        || !matches!(
            args.delivery_mode(),
            Ok(None | Some(DeliveryMode::NextTurn | DeliveryMode::Redirect))
        )
        || !(MIN_TIMEOUT..=MAX_TIMEOUT).contains(&timeout)
    {
        return result.unavailable(
            Error::invalid_request(concat!(
                "Blocking sends require no delay, next-turn or redirect ",
                "and a timeout from 1 to 600 seconds."
            ))
            .into(),
        );
    }
    let deadline = Instant::now() + Duration::from_secs(timeout);
    match exchange::<api::SendResponse>(root, id, &Request::Send(args), deadline).await {
        Ok(accepted) => observe(root, accepted.message_id, result, deadline, progress).await,
        Err(error) => result.unavailable(error),
    }
}

pub(crate) async fn wait(
    root: &Path,
    message: &str,
    timeout: u64,
    progress: impl FnMut(&CallResult),
) -> CallResult {
    let result = CallResult::new(Some(message.into()), None);
    if !(MIN_TIMEOUT..=MAX_TIMEOUT).contains(&timeout) {
        return result.unavailable(
            Error::invalid_request("Message wait timeout must be from 1 to 600 seconds.").into(),
        );
    }
    observe(
        root,
        message.into(),
        result,
        Instant::now() + Duration::from_secs(timeout),
        progress,
    )
    .await
}

async fn observe(
    root: &Path,
    message: String,
    mut result: CallResult,
    deadline: Instant,
    mut progress: impl FnMut(&CallResult),
) -> CallResult {
    result.message_id = Some(message.clone());
    let request = Request::MessageResult(api::MessageResultArgs {
        message_id: message.clone().into(),
    });
    // Stable per-message jitter prevents callers starting together from staying in lockstep.
    let jitter = message.bytes().map(u64::from).sum::<u64>() % JITTER_MILLIS;
    progress(&result); // Publish identity immediately, before the first result lookup.
    while Instant::now() < deadline {
        match exchange::<MessageResult>(root, "message-observer", &request, deadline).await {
            Ok(snapshot) => {
                if snapshot.message_id != message {
                    return result.unavailable(
                        Error::daemon_protocol_error("Daemon returned a different message result.")
                            .into(),
                    );
                }
                result.outcome = snapshot.outcome;
                result.result = Some(snapshot);
                if result.outcome != CallOutcome::Pending {
                    return result;
                }
                progress(&result);
            }
            Err(error) if error.is_deadline() => break,
            Err(error) => return result.unavailable(error),
        }
        tokio::time::sleep_until(
            deadline.min(Instant::now() + Duration::from_millis(POLL_MILLIS + jitter)),
        )
        .await;
    }
    result.outcome = CallOutcome::Timeout;
    result
}

pub(crate) fn cli_progress() -> impl FnMut(&CallResult) {
    progress_changes(|line| eprintln!("{line}"))
}

fn progress_changes(mut emit: impl FnMut(&str)) -> impl FnMut(&CallResult) {
    let mut previous = None;
    move |result| {
        if let Some(line) = progress_line(result)
            && previous.as_ref() != Some(&line)
        {
            emit(&line);
            previous = Some(line);
        }
    }
}

fn progress_line(result: &CallResult) -> Option<String> {
    if let Some(snapshot) = &result.result {
        let queue = snapshot.queued_ahead.map_or_else(String::new, |ahead| {
            format!(
                "; queued behind {}{} messages",
                ahead,
                if snapshot.queue_count_exact { "" } else { "+" },
            )
        });
        Some(format!(
            "Hive: message {} {}{}; active input: {}; recipient: {}{}.",
            snapshot.message_id,
            snapshot.delivery_state.as_str(),
            queue,
            snapshot.active_input,
            snapshot.recipient_state.as_str(),
            if snapshot.paused { " (paused)" } else { "" }
        ))
    } else {
        result
            .message_id
            .as_ref()
            .map(|id| format!("Hive: observing message {id}; resume with wait --message {id}."))
    }
}

pub(crate) fn successful(result: &CallResult) -> bool {
    result.outcome == CallOutcome::Replied
}

// Kept typed through CLI and MCP; raw JSON is created only at the display boundary.
pub(crate) fn value(result: CallResult) -> Value {
    api::value(result)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::runtime::domain::{AgentState, MessageState};

    #[test]
    fn blocking_send_rejects_delay_before_contacting_daemon() {
        let args = SendArgs {
            delay_seconds: 1.into(),
            ..Default::default()
        };
        let result =
            executor()
                .unwrap()
                .block_on(send(Path::new("/nonexistent"), "id", args, 1, |_| {}));
        assert_eq!(result.error.unwrap().code(), ErrorCode::InvalidRequest);
    }

    #[test]
    fn cli_progress_prints_only_identity_and_changed_queue_state() {
        let mut lines = Vec::new();
        let mut progress = progress_changes(|line| lines.push(line.to_owned()));
        let mut result = CallResult::new(Some("message".into()), None);
        progress(&result);
        progress(&result);
        result.result = Some(MessageResult {
            held_for_build: None,
            message_id: "message".into(),
            recipient: "agent".into(),
            delivery_state: MessageState::Accepted,
            not_before: None,
            outcome: CallOutcome::Pending,
            execution_id: None,
            thread_id: None,
            turn_id: None,
            events_after: None,
            output: None,
            output_selection: api::OutputSelection::NoText,
            output_partial: true,
            queued_ahead: Some(2),
            queue_count_exact: true,
            active_input: true,
            recipient_state: AgentState::Running,
            paused: false,
            delivery_error: None,
        });
        for _ in 0..600 {
            progress(&result);
        }
        result.result.as_mut().unwrap().queued_ahead = Some(1);
        progress(&result);
        result.result.as_mut().unwrap().queue_count_exact = false;
        progress(&result);
        result.result.as_mut().unwrap().paused = true;
        progress(&result);
        result.result.as_mut().unwrap().active_input = false;
        progress(&result);
        result.result.as_mut().unwrap().recipient_state = AgentState::Idle;
        progress(&result);
        result.result.as_mut().unwrap().delivery_state = MessageState::Submitting;
        result.result.as_mut().unwrap().queued_ahead = None;
        progress(&result);
        result.result.as_mut().unwrap().output = Some(serde_json::json!({"text":"partial"}));
        progress(&result);
        drop(progress);
        assert_eq!(lines.len(), 8);
        assert!(lines[0].contains("resume with wait --message message"));
        assert!(lines[1].contains("queued behind 2 messages"));
        assert!(lines[3].contains("queued behind 1+ messages"));
        assert!(lines[7].contains("submitting"));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Reply corruption is exercised before any daemon operation.
mod format_tests {
    use super::*;
    use serde_json::json;

    fn strict_reply<T: DeserializeOwned>(good: Value, enums: &[&str]) {
        assert!(matches!(
            decode::<T>(&good.to_string()),
            Ok(Reply::Success(_))
        ));
        for name in good.as_object().unwrap().keys() {
            let mut value = good.clone();
            value.as_object_mut().unwrap().remove(name);
            assert_eq!(
                decode::<T>(&value.to_string()).err().unwrap().code(),
                "daemon_protocol_error",
                "missing {name}"
            );
        }
        let mut unknown = good.clone();
        unknown["unexpected"] = json!(true);
        assert_eq!(
            decode::<T>(&unknown.to_string()).err().unwrap().code(),
            "daemon_protocol_error"
        );
        for name in enums {
            let mut value = good.clone();
            value[name] = json!("unsupported");
            assert_eq!(
                decode::<T>(&value.to_string()).err().unwrap().code(),
                "daemon_protocol_error",
                "unknown {name}"
            );
        }
    }

    #[test]
    fn observed_responses_require_current_fields_and_known_states() {
        strict_reply::<api::SendResponse>(
            json!({
                "message_id":"message", "delivery_state":"accepted", "recipient":"agent",
            }),
            &["delivery_state"],
        );
        strict_reply::<api::MessageResult>(
            json!({
                "not_before": null,
                "message_id": "message",
                "recipient": "agent",
                "delivery_state": "accepted",
                "outcome": "pending",
                "execution_id": null,
                "thread_id": null,
                "turn_id": null,
                "events_after": null,
                "output": null,
                "output_selection": "no_text",
                "output_partial": true,
                "queued_ahead": null,
                "queue_count_exact": true,
                "active_input": false,
                "recipient_state": "idle",
                "paused": false,
                "delivery_error": null,
            }),
            &[
                "delivery_state",
                "outcome",
                "output_selection",
                "recipient_state",
            ],
        );
        strict_reply::<api::OutputResponse>(
            json!({
                "artifact_id": "artifact",
                "offset": 0,
                "text": "text",
                "next_offset": 4,
                "bytes": 4,
                "eof": true,
            }),
            &[],
        );
        strict_reply::<crate::approvals::ApprovalsPage>(
            json!({"approvals":[],"next_after":null}),
            &[],
        );
    }

    #[test]
    fn observed_errors_reject_missing_unknown_and_nonobject_fields() {
        for value in [
            json!({"error": {"code":"provider_failed"}}),
            json!({"error": {"message":"failed"}}),
            json!({"error": {"code":"provider_failed","message":"failed","unexpected":true}}),
            json!({"error": {"code":"provider_failed","message":"failed"},"unexpected":true}),
            json!({"error":null}),
            json!([]),
        ] {
            assert_eq!(
                decode::<api::OutputResponse>(&value.to_string())
                    .err()
                    .unwrap()
                    .code(),
                "daemon_protocol_error"
            );
        }
        assert!(matches!(
            decode::<api::OutputResponse>(
                r#"{"error":{"code":"provider_failed","message":"failed"}}"#
            ),
            Ok(Reply::Error { .. })
        ));
    }
}
