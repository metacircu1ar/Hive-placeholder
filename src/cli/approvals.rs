//! Approval commands, bounded response input and human decision polling.
use super::{
    CommandOutcome, DEFAULT_WAIT_SECONDS, MAX_WAIT_SECONDS, WAIT_POLL_INTERVAL, remote_response,
};
use crate::runtime::limits::{DEFAULT_HISTORY_PAGE_ROWS, MAX_PAGE_ROWS, MIN_PAGE_ROWS};
use crate::{
    approvals::{ApprovalsPage, RESPONSE_BYTES},
    error::{Error, Result},
    runtime::api::{self, ApprovalAction, Input, Request},
};
use clap::{Args, Subcommand};
use serde::Serialize;
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Args)]
#[command(args_conflicts_with_subcommands = true)]
pub(super) struct ListArgs {
    #[command(subcommand)]
    command: Option<ShowCommand>,
    #[arg(long, value_parser = hive_input::clap::parsers::text(crate::limits::CONTROL_INPUT_BYTES))]
    /// Filter questions for this agent ID.
    agent: Option<String>,
    /// Include closed questions; pages reflect the live database.
    #[arg(long)]
    all: bool,
    /// JSON `next_after` object: `{"created_at":INTEGER,"id":"CURSOR_ID"}`.
    #[arg(long)]
    after: Option<String>,
    #[arg(
        long,
        default_value_t = DEFAULT_HISTORY_PAGE_ROWS,
        value_parser = clap::value_parser!(u64).range(MIN_PAGE_ROWS..=MAX_PAGE_ROWS)
    )]
    /// Maximum questions per page (1-200; default 100).
    limit: u64,
    /// Poll the first pending page until a question is available.
    #[arg(long, conflicts_with_all = ["all", "after"])]
    wait: bool,
    #[arg(
        long,
        requires = "wait",
        value_parser = clap::value_parser!(u64).range(1..=MAX_WAIT_SECONDS)
    )]
    /// Wait deadline in seconds (1-3600; default 60); requires --wait.
    timeout: Option<u64>,
}

#[derive(Subcommand)]
enum ShowCommand {
    /// Inspect one pending or closed approval question.
    Show {
        #[arg(value_parser = hive_input::clap::parsers::uuid)]
        /// Approval question UUID.
        approval_id: String,
    },
}

#[derive(Args)]
#[command(group(
    clap::ArgGroup::new("action")
        .required(true)
        .multiple(false)
        .args(["accept", "decline", "cancel"])
))]
pub(super) struct RespondArgs {
    #[arg(value_parser = hive_input::clap::parsers::uuid)]
    /// Approval question UUID.
    approval_id: String,
    #[arg(long)]
    /// Accept the question, optionally supplying form data.
    accept: bool,
    #[arg(long)]
    /// Decline the question, optionally supplying a reason.
    decline: bool,
    #[arg(long)]
    /// Cancel the pending question.
    cancel: bool,
    #[arg(long, requires = "accept", conflicts_with = "data_file")]
    /// Accepted form fields as a JSON object; requires --accept.
    data: Option<String>,
    #[arg(long, requires = "accept")]
    /// UTF-8 JSON form file (at most 61440 bytes); requires --accept.
    data_file: Option<PathBuf>,
    #[arg(long, requires = "decline")]
    /// Optional explanation when declining the question.
    reason: Option<String>,
    #[arg(long, value_parser = hive_input::clap::parsers::request_id)]
    /// Stable retry key (1-128 bytes, no controls); reuse with identical input.
    pub(super) request_id: Option<String>,
}

impl ListArgs {
    pub(super) fn request(&self) -> super::input::Result<Request> {
        if let Some(ShowCommand::Show { approval_id }) = &self.command {
            return Ok(Request::Approval(api::ApprovalArgs {
                approval_id: approval_id.clone().into(),
            }));
        }
        let context = super::input::Context::new("approvals", "--after");
        let after: Option<api::ApprovalCursor> = self
            .after
            .as_ref()
            .map(|value| {
                // JSON parser diagnostics may retain supplied cursor values. Keep only
                // trusted field context, never the original parsing cause.
                serde_json::from_str(value).map_err(|_| {
                    context.invalid(
                        hive_input::Code::InvalidJson,
                        "Invalid approval cursor.",
                        "a JSON object with created_at (integer) and id (string)",
                    )
                })
            })
            .transpose()?;
        let request = api::ApprovalsArgs {
            agent: self.agent.clone().map_or(Input::Missing, Input::from),
            all: self.all.into(),
            after: after.map_or(Input::Missing, Input::from),
            limit: self.limit.into(),
        };
        request.options().map_err(|error| match error {
            Error::Input { input, .. } => {
                let Some(flag) = list_field(input.field()) else {
                    return super::input::Failure::from(Error::internal_error(
                        "Missing approval input context.",
                    ));
                };
                super::input::Context::new("approvals", flag).invalid(
                    input.code(),
                    "Invalid approval arguments.",
                    input.expected(),
                )
            }
            error => error.into(),
        })?;
        Ok(Request::Approvals(request))
    }
    pub(super) fn run(&self, root: &Path, id: &str, request: Request) -> Result<CommandOutcome> {
        if self.command.is_some() || !self.wait {
            return remote_response(root, id, request);
        }
        crate::calls::executor()?.block_on(wait(
            root,
            id,
            &request,
            self.timeout.unwrap_or(DEFAULT_WAIT_SECONDS),
        ))
    }
}

pub(super) fn list_field(pointer: &str) -> Option<&'static str> {
    match pointer {
        "/agent" => Some("--agent"),
        "/all" => Some("--all"),
        "/limit" => Some("--limit"),
        "/after" | "/after/created_at" | "/after/id" => Some("--after"),
        _ => None,
    }
}

#[derive(Serialize)]
struct ObservationFailure<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<&'a str>,
    error: crate::calls::ObservationError,
}

async fn wait(root: &Path, id: &str, request: &Request, timeout: u64) -> Result<CommandOutcome> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout);
    loop {
        let page = crate::calls::exchange::<ApprovalsPage>(root, id, request, deadline).await;
        match page {
            Ok(page) if !page.approvals.is_empty() => {
                return Ok(CommandOutcome::ok(api::value(page)));
            }
            Err(error) if !error.is_deadline() => {
                let request_id = error.input().map(|_| id);
                let value = api::value(ObservationFailure { request_id, error });
                // Observation errors deliberately allow empty operational messages.
                // Only typed input rejections use the command diagnostic adapter.
                if request_id.is_none() {
                    return Ok(CommandOutcome::failed(value));
                }
                let reply = api::DaemonReply::new(&value)?;
                let input = super::remote::input_reply(&reply, request)?;
                let value = input
                    .as_ref()
                    .map_or(value, super::remote::InputReply::value);
                return Ok(CommandOutcome {
                    value,
                    success: false,
                    input,
                });
            }
            _ => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(CommandOutcome::failed(api::value(api::ErrorResponse {
                error: Error::timeout("No pending approval appeared before the timeout."),
            })));
        }
        tokio::time::sleep_until((tokio::time::Instant::now() + WAIT_POLL_INTERVAL).min(deadline))
            .await;
    }
}

pub(super) fn response_request(args: &RespondArgs) -> super::input::Result<Request> {
    let action = if args.accept {
        ApprovalAction::Accept
    } else if args.decline {
        ApprovalAction::Decline
    } else {
        ApprovalAction::Cancel
    };
    if let Some(reason) = &args.reason {
        super::input::Context::new("respond", "--reason").checked_text(
            reason.clone(),
            RESPONSE_BYTES,
            true,
        )?;
    }
    let content = content(args)?;
    Ok(Request::Respond(api::RespondArgs {
        approval_id: args.approval_id.clone().into(),
        action: action.into(),
        content: content.map_or(Input::Missing, Input::from),
        reason: args.reason.clone().map_or(Input::Missing, Input::from),
    }))
}

fn content(args: &RespondArgs) -> super::input::Result<Option<Value>> {
    use super::input::Context;
    let file_bytes;
    let context;
    let bytes = if let Some(path) = &args.data_file {
        context = Context::new("respond", "--data-file").file(path);
        file_bytes = context.bytes(RESPONSE_BYTES)?;
        file_bytes.as_slice()
    } else if let Some(data) = &args.data {
        context = Context::new("respond", "--data");
        data.as_bytes()
    } else {
        return Ok(None);
    };
    context.json(bytes, RESPONSE_BYTES).map(Some)
}

impl RespondArgs {
    pub(super) fn run(&self, root: &Path, id: &str, request: Request) -> Result<CommandOutcome> {
        let mut outcome = remote_response(root, id, request)?;
        if let Some(input) = outcome.input.take() {
            let input = input.file_source(
                "respond",
                "--data",
                "--data-file",
                self.data_file.as_deref(),
            )?;
            outcome.value = input.value();
            outcome.input = Some(input);
        }
        Ok(outcome)
    }
}
