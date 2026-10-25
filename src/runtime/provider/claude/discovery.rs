//! Discover the selected Claude authentication method and native model catalog.
use super::{
    Cancellation, Captured, Command, Deserialize, Duration, Error, Initialization, Operation,
    Result, Value, wire,
};

use super::wire::{catalog as output, control};
use crate::runtime::{api, provider::limits::NATIVE_EOF_GRACE};
use crate::{
    launch::LauncherCache,
    process::malformed,
    state::{Binding, State},
};
use std::os::fd::BorrowedFd;

// Bound authentication/catalog initialization without running an inference turn.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(30);
// Permit startup notices while bounding the search for the initialize response.
const INITIALIZATION_FRAME_LIMIT: usize = 32;
// Reject unbounded native catalogs before alias expansion or cache accounting.
const NATIVE_MODEL_LIMIT: usize = 1000;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum DiscoveryFrame {
    ControlResponse { response: wire::ControlResult },
    RateLimitEvent,
    System { subtype: wire::SystemSubtype },
}

pub(crate) fn stream_arguments(command: &mut Command) {
    command.args([
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-prompt-tool",
        "stdio",
        "--strict-mcp-config",
        "--mcp-config",
        "{\"mcpServers\":{}}",
        "--disable-slash-commands",
        "--no-chrome",
    ]);
}

pub(crate) fn discover(
    launcher: &LauncherCache,
    mut command: Command,
    operation: &mut Operation,
    state: &State,
    binding: &Binding,
    owner: Option<BorrowedFd<'_>>,
) -> Result<Value> {
    stream_arguments(&mut command);
    let mut child = Captured::start_with_owner(
        launcher,
        &mut command,
        operation,
        DISCOVERY_TIMEOUT,
        Cancellation::default(),
        owner,
    )?;
    child.send(&api::value(control::OutboundControl {
        kind: "control_request",
        request_id: "hive-initialize",
        request: control::NativeRequest::discovery(),
    }))?;
    let init = initialization(&mut child)?;
    if !init.account.matches(binding.profile.method) {
        return Err(Error::authentication_required(
            "Claude account does not match the selected authentication method.",
        ));
    }
    if binding.profile.method == crate::config::Method::Subscription {
        state.check_account(binding, init.account.email.as_deref())?;
    }
    let catalog = catalog(&init)?;
    // Match the runtime provider's EOF grace; native Claude can take more than
    // the short auth-helper grace to finish a healthy no-inference session.
    let Some(output) = child.finish_with_grace(NATIVE_EOF_GRACE)? else {
        return Err(Error::provider_shutdown_forced(
            "Claude discovery did not exit cleanly.",
        ));
    };
    if !output.status.success() {
        return Err(Error::provider_failed(
            "Claude discovery exited unsuccessfully.",
        ));
    }
    Ok(catalog)
}

fn initialization(child: &mut Captured) -> Result<Initialization> {
    let mut result = None;
    for _ in 0..INITIALIZATION_FRAME_LIMIT {
        let frame: DiscoveryFrame = serde_json::from_value(child.next_json()?)
            .map_err(|cause| malformed().with_source(cause))?;
        match frame {
            DiscoveryFrame::ControlResponse { response }
                if response.request_id == "hive-initialize" =>
            {
                result = Some(response);
                break;
            }
            DiscoveryFrame::RateLimitEvent
            | DiscoveryFrame::System {
                subtype: wire::SystemSubtype::Init | wire::SystemSubtype::Status,
            } => {}
            _ => return Err(malformed()),
        }
    }
    let reply = result.ok_or_else(malformed)?;
    if reply.subtype != wire::ControlSubtype::Success {
        return Err(Error::provider_request_failed(
            "Claude initialization failed.",
        ));
    }
    let init: Initialization =
        serde_json::from_value(reply.response).map_err(|cause| malformed().with_source(cause))?;
    Ok(init)
}

pub(crate) fn catalog(init: &Initialization) -> Result<Value> {
    if init.models.is_empty() || init.models.len() > NATIVE_MODEL_LIMIT {
        return Err(malformed());
    }
    let data = init.models.iter().try_fold(Vec::new(), |data, model| {
        if model.value.is_empty() || model.resolved.is_empty() {
            return Err(malformed());
        }
        let efforts: Vec<output::Effort> = if model.supported_effort_levels.is_empty() {
            vec![output::Effort {
                reasoning_effort: "none",
            }]
        } else {
            model
                .supported_effort_levels
                .iter()
                .map(|effort| output::Effort {
                    reasoning_effort: effort,
                })
                .collect()
        };
        Ok([&model.value, &model.resolved]
            .into_iter()
            .fold(data, |mut data, name| {
                if !data.iter().any(|entry: &output::Entry| entry.model == name) {
                    data.push(output::Entry {
                        id: name,
                        model: name,
                        resolved_model: &model.resolved,
                        display_name: &model.display_name,
                        description: &model.description,
                        supported_reasoning_efforts: efforts.clone(),
                    });
                }
                data
            }))
    })?;
    Ok(api::value(output::Catalog {
        data,
        next_cursor: None,
    }))
}
