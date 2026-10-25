//! Human event following with bounded cursor reads and synchronous output backpressure.
use super::{Cli, DEFAULT_EVENT_PAGE_ROWS, Error, ReadArgs, Request, Result, api};
use crate::runtime;
use serde_json::Value;
use std::{io::Write, path::Path, time::Duration};

// Idle human viewers make at most two history requests per second.
const FOLLOW_POLL_INTERVAL: Duration = Duration::from_millis(500);

#[derive(clap::Args)]
pub(super) struct Read {
    #[arg(value_parser = hive_input::clap::parsers::text(crate::limits::CONTROL_INPUT_BYTES))]
    /// Agent ID, artifact:UUID, or join:ID to read.
    target: String,
    /// Exclusive event cursor or artifact byte offset (default 0); omit for joins.
    #[arg(long)]
    after: Option<u64>,
    /// Rows: 1–200 (default 100). Artifact bytes: 4–65536 (default 65536). Omit for joins.
    #[arg(long)]
    limit: Option<u64>,
    /// Watch this agent as JSONL pages until Ctrl-C; intended for humans only.
    #[arg(long)]
    pub(super) follow: bool,
}

impl Read {
    pub(super) fn request(&self) -> Request {
        Request::Read(self.arguments())
    }

    fn arguments(&self) -> ReadArgs {
        ReadArgs {
            target: self.target.clone().into(),
            after: self.after.map_or(api::Input::Missing, api::Input::Valid),
            limit: self.limit.map_or(api::Input::Missing, api::Input::Valid),
        }
    }

    fn request_after(&self, after: u64) -> Request {
        Request::Read(ReadArgs {
            target: self.target.clone().into(),
            after: after.into(),
            limit: self.limit.unwrap_or(DEFAULT_EVENT_PAGE_ROWS).into(),
        })
    }

    pub(super) fn validate(&self) -> std::result::Result<(), api::pagination::InvalidPage> {
        if self.follow && (self.target.starts_with("artifact:") || self.target.starts_with("join:"))
        {
            return Err(api::pagination::InvalidPage {
                code: hive_input::Code::ConflictingArguments,
                field: "/follow",
                expected: "an agent target when --follow is present",
            });
        }
        self.arguments().page().map(|_| ())
    }
}

pub(super) fn run(cli: &Cli, args: &Read) -> i32 {
    // Pure argument validation has already run through the shared CLI parser.
    let result = cli
        .state_root()
        .and_then(|root| follow(&root, args, &mut std::io::stdout().lock()));
    match result {
        Ok(None) => 0, // The consumer closed its output pipe.
        Ok(Some(Failure::Input(input))) => input.emit(),
        Ok(Some(Failure::Operational { value, code })) => super::parsing::emit_failure_record(
            &value,
            format!("Hive: cannot follow agent events ({}).\n", code.as_str()),
        ),
        Ok(Some(Failure::Output(error))) => {
            super::parsing::emit_stderr(super::parsing::human(&error))
        }
        Err(error) => {
            let human = super::parsing::human(&error);
            super::parsing::emit_failure_record(&api::value(api::ErrorResponse { error }), human)
        }
    }
}

enum Failure {
    Input(super::remote::InputReply),
    Operational {
        value: Value,
        code: crate::error::ErrorCode,
    },
    Output(Error),
}

fn follow(root: &Path, args: &Read, output: &mut impl Write) -> Result<Option<Failure>> {
    let mut after = args.after.unwrap_or(0);
    loop {
        let request = args.request_after(after);
        let response = runtime::client(root, &uuid::Uuid::new_v4().to_string(), &request)?;
        let reply = api::DaemonReply::new(&response)?;
        if let Some(input) = super::remote::input_reply(&reply, &request)? {
            return Ok(Some(Failure::Input(input)));
        }
        if let Some(code) = reply.error_code() {
            return Ok(Some(Failure::Operational {
                value: response,
                code,
            }));
        }
        let page = reply.events()?;
        page.validate_follow(
            &args.target,
            after,
            args.limit.unwrap_or(DEFAULT_EVENT_PAGE_ROWS),
        )?;
        if (!page.is_empty() || page.history_expired())
            && let Err(error) = writeln!(output, "{response}").and_then(|()| output.flush())
        {
            if error.kind() == std::io::ErrorKind::BrokenPipe {
                return Ok(None);
            }
            return Ok(Some(Failure::Output(
                Error::state_io("Cannot write command output.").with_source(error),
            )));
        }
        // Never advance or request another page before successful output flush.
        after = page.cursor();
        // Byte-budget truncation can produce a short page while more events remain.
        if page.is_empty() {
            std::thread::sleep(FOLLOW_POLL_INTERVAL);
        }
    }
}

#[cfg(test)]
mod tests;
