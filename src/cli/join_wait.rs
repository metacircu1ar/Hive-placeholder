//! External-only bounded observation of a durable join; no writer is held waiting.
use super::{CommandOutcome, Error, Input, ReadArgs, Request, Result, api};
use crate::runtime::api::joins::{JoinSnapshot, JoinState};
use serde::Serialize;
use std::{path::Path, time::Duration};
use tokio::time::Instant;

#[derive(Serialize)]
struct Observation {
    wait: &'static str,
    join_id: String,
    join: Option<JoinSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<crate::calls::ObservationError>,
}
pub(super) fn run(root: &Path, id: &str, timeout: u64) -> Result<CommandOutcome> {
    crate::calls::executor()?.block_on(observe(root, id, timeout))
}
async fn observe(root: &Path, id: &str, timeout: u64) -> Result<CommandOutcome> {
    let deadline = Instant::now() + Duration::from_secs(timeout);
    let request = Request::Read(ReadArgs {
        target: format!("join:{id}").into(),
        after: Input::Missing,
        limit: Input::Missing,
    });
    let mut last = None;
    loop {
        match crate::calls::exchange::<JoinSnapshot>(
            root,
            &uuid::Uuid::new_v4().to_string(),
            &request,
            deadline,
        )
        .await
        {
            Ok(join) => {
                if join.join_id != id {
                    return Ok(observation(
                        id,
                        "observation_unavailable",
                        last,
                        Some(
                            Error::daemon_protocol_error(
                                "Daemon returned a different join result.",
                            )
                            .into(),
                        ),
                    ));
                }
                if join.closed() {
                    return Ok(CommandOutcome {
                        input: None,
                        success: join.state == JoinState::Completed,
                        value: api::value(join),
                    });
                }
                if join.held_for_build.is_some() {
                    return Ok(observation(id, "held", Some(join), None));
                }
                last = Some(join);
            }
            Err(error) => {
                if last.is_none()
                    && let Some(input) = error.input()
                {
                    return Err(initial_input(input));
                }
                let wait = if error.is_deadline() {
                    "timeout"
                } else {
                    "observation_unavailable"
                };
                return Ok(observation(id, wait, last, Some(error)));
            }
        }
        if Instant::now() >= deadline {
            return Ok(observation(id, "timeout", last, None));
        }
        tokio::time::sleep_until((Instant::now() + OBSERVATION_INTERVAL).min(deadline)).await;
    }
}

fn observation(
    id: &str,
    wait: &'static str,
    join: Option<JoinSnapshot>,
    error: Option<crate::calls::ObservationError>,
) -> CommandOutcome {
    CommandOutcome::failed(api::value(Observation {
        wait,
        join_id: id.into(),
        join,
        error,
    }))
}

// A bad initial selector is caller input. Loss after observation remains an
// observation failure and retains the last snapshot in the ordinary outcome.
fn initial_input(input: &hive_input::InputError) -> Error {
    if input.field() != "/target" {
        return crate::runtime::client::envelope::invalid();
    }
    let Some(examples) = super::catalog::examples("wait") else {
        return Error::internal_error("Missing CLI input examples.");
    };
    hive_input::InputError::new(
        input.code(),
        input.message(),
        "--join",
        input.expected(),
        examples.as_slice().to_vec(),
    )
    .map_or_else(
        |cause| crate::runtime::client::envelope::invalid().with_source(cause),
        Error::from,
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;

const OBSERVATION_INTERVAL: std::time::Duration = Duration::from_millis(500);
