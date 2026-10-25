//! Send input once, optionally observing the exact consuming turn.
use super::{
    CommandOutcome, DeliveryMode, Input, Path, PathBuf, Request, Result, SendArgs, duration,
    remote_response,
};
use crate::calls;
#[derive(clap::Args)]
pub(super) struct Send {
    #[arg(value_parser = hive_input::clap::parsers::text(crate::limits::CONTROL_INPUT_BYTES))]
    /// Agent ID to operate on.
    target: String,
    /// Deliver as next-turn input after this duration (1s through 7d).
    #[arg(long, conflicts_with = "wait", value_parser = duration::delay)]
    pub(super) delay: Option<u64>,
    /// Message as literal text; exactly one body source is required.
    #[arg(
        long,
        allow_hyphen_values = true,
        required_unless_present = "body_file",
        conflicts_with = "body_file"
    )]
    body: Option<String>,
    #[arg(long, required_unless_present = "body", conflicts_with = "body")]
    /// UTF-8 initial/message text file (at most 65536 bytes).
    body_file: Option<PathBuf>,
    /// Attach a file snapshot (repeatable, at most 10); oversized files stay live paths.
    #[arg(long = "attach", value_name = "PATH")]
    attachments: Vec<PathBuf>,
    #[arg(long, value_parser = hive_input::clap::parsers::request_id)]
    /// Stable retry key (1-128 bytes, no controls); reuse with identical input.
    pub(super) request_id: Option<String>,
    /// Wait for this exact message's consuming turn. Timeout does not cancel it.
    #[arg(long)]
    wait: bool,
    #[arg(
        long,
        requires = "wait",
        default_value_t = calls::DEFAULT_TIMEOUT,
        value_parser = clap::value_parser!(u64).range(calls::MIN_TIMEOUT..=calls::MAX_TIMEOUT)
    )]
    /// Wait deadline in seconds (1-600; default 45); requires --wait.
    timeout: u64,
    /// Queue input, steer the active turn, or interrupt it and redirect the next turn.
    #[arg(long, default_value = "next-turn", value_enum)]
    mode: DeliveryMode,
}

impl Send {
    /// These relationships depend on an enum value, so check after clap decoding
    /// and before reading body/attachment files or emitting blocking-call progress.
    pub(super) fn invalid_mode(&self) -> Option<&'static str> {
        if self.delay.is_some() && self.mode != DeliveryMode::NextTurn {
            Some("next-turn when --delay is present")
        } else if self.wait && self.mode == DeliveryMode::Steer {
            Some("next-turn or redirect when --wait is present")
        } else {
            None
        }
    }

    pub(super) fn request(&self) -> super::input::Result<Request> {
        Ok(Request::Send(SendArgs {
            attachments: super::input::attachment_paths("send", &self.attachments)?,
            delay_seconds: self.delay.map_or(Input::Missing, Input::Valid),
            target: self.target.clone().into(),
            body: super::input::text_source(
                "send",
                "--body",
                "--body-file",
                self.body.as_deref(),
                self.body_file.as_deref(),
            )?
            .into(),
            mode: self.mode.into(),
        }))
    }

    pub(super) fn run(
        &self,
        root: &Path,
        request_id: &str,
        args: SendArgs,
    ) -> Result<CommandOutcome> {
        if self.wait {
            eprintln!(
                concat!(
                    "Hive: request ID {}; reuse this ID and send arguments ",
                    "if acceptance is lost."
                ),
                request_id
            );
            let result = calls::executor()?.block_on(calls::send(
                root,
                request_id,
                args,
                self.timeout,
                calls::cli_progress(),
            ));
            return Ok(result.into());
        }
        let mut outcome = remote_response(root, request_id, Request::Send(args))?;
        if let Some(input) = outcome.input.take() {
            let input =
                input.file_source("send", "--body", "--body-file", self.body_file.as_deref())?;
            outcome.value = input.value();
            outcome.input = Some(input);
        }
        Ok(outcome)
    }
}
