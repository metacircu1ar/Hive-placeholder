//! Prepare batch bounds before startup and retain private local file provenance.
use super::{CommandOutcome, Input, Path, PathBuf, Request, Result, api, input};

#[derive(clap::Args)]
pub(super) struct Batch {
    /// Collect the final output of each message turn into one durable result.
    #[arg(long)]
    join: bool,
    /// Partial-result delay; current Unix time plus seconds must fit an `i64`.
    #[arg(long, requires = "join",
         value_parser = clap::value_parser!(u64).range(1..=i64::MAX as u64))]
    deadline_seconds: Option<u64>,
    /// JSON array of 1–64 spawn/send/terminate operations, each with `request_id`.
    #[arg(long)]
    operations_file: PathBuf,
    #[arg(long, value_parser = hive_input::clap::parsers::request_id)]
    /// Stable retry key (1-128 bytes, no controls); reuse with identical input.
    pub(super) request_id: Option<String>,
}

impl Batch {
    pub(super) fn request(&self) -> input::Result<Request> {
        let join = if self.join {
            let options = api::joins::JoinOptions {
                deadline_seconds: self.deadline_seconds.map_or(Input::Missing, Input::Valid),
            };
            options
                .deadline(crate::state::now())
                .map_err(|error| local(error, None))?;
            options.into()
        } else {
            Input::Missing
        };
        let request = Request::Batch(api::BatchArgs {
            join,
            operations: Input::from_value(super::batch_operations(&self.operations_file)?),
        });
        api::RequestEnvelope::new(api::value(&request))
            .checked_batch_items()
            .map_err(|error| local(error, Some(&self.operations_file)))?;
        Ok(request)
    }

    pub(super) fn run(&self, root: &Path, id: &str, request: Request) -> Result<CommandOutcome> {
        let mut outcome = super::remote_response(root, id, request)?;
        if let Some(input) = outcome.input.take() {
            let input = input.file_source(
                "batch",
                "--operations-file",
                "--operations-file",
                Some(&self.operations_file),
            )?;
            outcome.value = input.value();
            outcome.input = Some(input);
        }
        Ok(outcome)
    }
}

pub(super) fn local(error: super::Error, path: Option<&Path>) -> input::Failure {
    let super::Error::Input { input, .. } = &error else {
        return error.into();
    };
    let Some(flag) = field(input.field()) else {
        return super::Error::internal_error("Missing batch input context.").into();
    };
    let mut context = input::Context::new("batch", flag);
    if flag == "--operations-file"
        && let Some(path) = path
    {
        context = context.file(path);
    }
    context.invalid(
        input.code(),
        "Invalid batch arguments.",
        &format!("{}: {}", input.field(), input.expected()),
    )
}

/// Accept canonical bounded indices and only fields emitted by batch validators.
pub(super) fn field(pointer: &str) -> Option<&'static str> {
    match pointer {
        "/join" => return Some("--join"),
        "/join/deadline_seconds" => return Some("--deadline-seconds"),
        "/operations" => return Some("--operations-file"),
        _ => {}
    }
    let indexed = pointer.strip_prefix("/operations/")?;
    let (index, field) = indexed.split_once('/').unwrap_or((indexed, ""));
    let position = index.parse::<usize>().ok()?;
    if position >= crate::runtime::limits::BATCH_OPERATION_LIMIT || position.to_string() != index {
        return None;
    }
    matches!(
        field,
        "" | "op"
            | "request_id"
            | "mode"
            | "target"
            | "body"
            | "role"
            | "name"
            | "from_agent"
            | "cwd"
            | "auth"
            | "provider"
            | "sandbox"
            | "lifetime"
            | "api_fallback"
            | "model"
            | "effort"
            | "instructions"
            | "attachments"
    )
    .then_some("--operations-file")
}
