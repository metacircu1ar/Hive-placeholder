//! Human live viewing: one attach snapshot, then long-polled live pages.
use super::{Cli, Error, Request, Result, api};
use crate::runtime;
use serde_json::Value;
use std::{io::Write, path::Path, time::Duration};

// A refused long-poll waits this long before trying again.
const CAPACITY_BACKOFF: Duration = Duration::from_secs(1);

#[derive(clap::Args)]
pub(super) struct Live {
    #[arg(value_parser = hive_input::clap::parsers::text(crate::limits::CONTROL_INPUT_BYTES))]
    /// Agent ID to watch.
    target: String,
    /// Print live pages as JSONL until Ctrl-C after the snapshot; for humans only.
    #[arg(long)]
    pub(super) follow: bool,
    /// Longest wait for each followed live page, 0–25000 milliseconds (default 25000).
    #[arg(long, value_parser = clap::value_parser!(u64).range(0..=25_000))]
    wait_ms: Option<u64>,
}

impl Live {
    pub(super) fn request(&self) -> Request {
        Request::Attach(api::AttachArgs {
            target: self.target.clone().into(),
            ..api::AttachArgs::default()
        })
    }
    fn next(&self, after: runtime::live::Cursor) -> Request {
        Request::Live(api::LiveArgs {
            target: self.target.clone().into(),
            after: after.into(),
            wait_ms: self.wait_ms.map_or(api::Input::Missing, api::Input::Valid),
            limit: api::Input::Missing,
        })
    }
}

pub(super) fn run(cli: &Cli, args: &Live) -> i32 {
    let result = cli
        .state_root()
        .and_then(|root| follow(&root, args, &mut std::io::stdout().lock()));
    match result {
        Ok(None) => 0, // The consumer closed its output pipe.
        Ok(Some(Failure::Input(input))) => input.emit(),
        Ok(Some(Failure::Operational { value, code })) => super::parsing::emit_failure_record(
            &value,
            format!(
                "Hive: cannot follow live agent output ({}).\n",
                code.as_str()
            ),
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

enum Reply {
    Value(Value),
    Busy,
    Failed(Failure),
}

fn call(root: &Path, request: &Request) -> Result<Reply> {
    let response = runtime::client(root, &uuid::Uuid::new_v4().to_string(), request)?;
    let reply = api::DaemonReply::new(&response)?;
    if let Some(input) = super::remote::input_reply(&reply, request)? {
        return Ok(Reply::Failed(Failure::Input(input)));
    }
    if let Some(code) = reply.error_code() {
        if code == crate::error::ErrorCode::WatchCapacity {
            return Ok(Reply::Busy);
        }
        return Ok(Reply::Failed(Failure::Operational {
            value: response,
            code,
        }));
    }
    Ok(Reply::Value(response))
}

fn cursor(value: &Value) -> Result<Option<runtime::live::Cursor>> {
    match value {
        Value::Null => Ok(None),
        value => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|cause| {
                Error::daemon_protocol_error("Invalid live cursor.").with_source(cause)
            }),
    }
}

/// Print one JSONL line; `false` when the consumer closed its pipe.
fn emit(output: &mut impl Write, value: &Value) -> std::result::Result<bool, Error> {
    match writeln!(output, "{value}").and_then(|()| output.flush()) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(false),
        Err(error) => Err(Error::state_io("Cannot write command output.").with_source(error)),
    }
}

fn attach(root: &Path, args: &Live, output: &mut impl Write) -> Result<Step> {
    loop {
        match call(root, &args.request())? {
            Reply::Busy => std::thread::sleep(CAPACITY_BACKOFF),
            Reply::Failed(failure) => return Ok(Step::Stop(Some(failure))),
            Reply::Value(value) => {
                let metadata: super::views::AttachMetadata<'_> = super::views::read(&value);
                let live = cursor(&metadata.live)?
                    .ok_or_else(|| Error::daemon_protocol_error("Attach returned no cursor."))?;
                return Ok(match emit(output, &value) {
                    Ok(true) => Step::Continue(live),
                    Ok(false) => Step::Stop(None),
                    Err(error) => Step::Stop(Some(Failure::Output(error))),
                });
            }
        }
    }
}

enum Step {
    Continue(runtime::live::Cursor),
    Stop(Option<Failure>),
}

fn follow(root: &Path, args: &Live, output: &mut impl Write) -> Result<Option<Failure>> {
    let mut after = match attach(root, args, output)? {
        Step::Continue(cursor) => cursor,
        Step::Stop(failure) => return Ok(failure),
    };
    if !args.follow {
        return Ok(None);
    }
    loop {
        let page = match call(root, &args.next(after))? {
            Reply::Busy => {
                std::thread::sleep(CAPACITY_BACKOFF);
                continue;
            }
            Reply::Failed(failure) => return Ok(Some(failure)),
            Reply::Value(page) => page,
        };
        let metadata: super::views::LivePage<Vec<serde::de::IgnoredAny>> =
            super::views::read(&page);
        let changed = metadata.resync || metadata.durable_changed || !metadata.items.is_empty();
        if changed {
            match emit(output, &page) {
                Ok(true) => {}
                Ok(false) => return Ok(None),
                Err(error) => return Ok(Some(Failure::Output(error))),
            }
        }
        // Never request another page before the previous one is written.
        after = match cursor(&metadata.cursor)? {
            Some(cursor) => cursor,
            // The channel expired or the daemon restarted: attach again.
            None => match attach(root, args, output)? {
                Step::Continue(cursor) => cursor,
                Step::Stop(failure) => return Ok(failure),
            },
        };
    }
}
