//! External MCP tools and their typed argument schemas.
// Output paging uses the same bounded observation deadline as other external tools.
use super::wire::{self, ProgressToken};
mod input;
use crate::{
    calls,
    runtime::{
        api::{
            CallOutcome, Input, OutputArgs, OutputResponse, Request, SendArgs, value as project,
        },
        domain::{DeliveryMode, wire_enum},
    },
};
pub(super) use input::{ParseError, parse};
use serde_json::Value;
use std::{path::Path, time::Duration};
const OUTPUT_TIMEOUT: Duration = Duration::from_secs(calls::DEFAULT_TIMEOUT);

pub(super) use super::catalog::catalog;

wire_enum!(ExternalTool {
    Call => "hive_call",
    WaitMessage => "hive_wait_message",
    Output => "hive_output",
});

struct CallArgs {
    target: String,
    body: String,
    request_id: String,
    attachments: Vec<std::path::PathBuf>,
    mode: DeliveryMode,
    timeout: u64,
}
struct WaitArgs {
    message_id: String,
    timeout: u64,
}
struct Output {
    id: String,
    offset: u64,
    limit: u64,
}
enum Operation {
    Call(CallArgs),
    Wait(WaitArgs),
    Output(Output),
}
pub(super) struct ToolCall {
    operation: Operation,
    pub progress_token: Option<ProgressToken>,
}

pub(super) struct ToolResult {
    pub value: Value,
    pub error: bool,
}
impl ToolCall {
    pub(super) async fn execute(
        self,
        root: &Path,
        progress: impl FnMut(&calls::CallResult),
    ) -> ToolResult {
        let result = match self.operation {
            Operation::Call(args) => {
                calls::send(
                    root,
                    &args.request_id,
                    SendArgs {
                        attachments: if args.attachments.is_empty() {
                            Input::Missing
                        } else {
                            Input::Valid(args.attachments)
                        },
                        delay_seconds: Input::Missing,
                        target: args.target.into(),
                        body: args.body.into(),
                        mode: args.mode.into(),
                    },
                    args.timeout,
                    progress,
                )
                .await
            }
            Operation::Wait(args) => {
                calls::wait(root, &args.message_id, args.timeout, progress).await
            }
            Operation::Output(args) => {
                let request = Request::Output(OutputArgs {
                    id: args.id.into(),
                    offset: args.offset.into(),
                    limit: Input::Valid(args.limit),
                });
                return match calls::exchange::<OutputResponse>(
                    root,
                    "external-output",
                    &request,
                    tokio::time::Instant::now() + OUTPUT_TIMEOUT,
                )
                .await
                {
                    Ok(value) => ToolResult {
                        value: project(value),
                        error: false,
                    },
                    Err(error) => ToolResult {
                        value: wire::tool_error(error),
                        error: true,
                    },
                };
            }
        };
        let error = matches!(result.outcome, CallOutcome::ObservationUnavailable);
        ToolResult {
            value: calls::value(result),
            error,
        }
    }
}

#[cfg(test)]
mod tests;
