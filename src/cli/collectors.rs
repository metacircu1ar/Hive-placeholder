use super::input::{Context, Result};
use super::{PathBuf, Request};
use crate::runtime::collectors::{Admin, EvidenceKind};
#[derive(clap::Args)]
pub(super) struct Evidence {
    #[arg(value_enum)]
    /// Public evidence kind: correction or review.
    kind: EvidenceKind,
    #[arg(
        long,
        allow_hyphen_values = true,
        conflicts_with = "summary_file",
        required_unless_present = "summary_file"
    )]
    /// Public nonblank UTF-8 text, at most 8192 bytes; no private markers.
    summary: Option<String>,
    #[arg(long)]
    /// Read the public summary once from a UTF-8 file (at most 8192 bytes).
    summary_file: Option<PathBuf>,
    #[arg(long = "file")]
    /// Public relative label, at most 512 bytes; repeat up to 64 times (no file read).
    files: Vec<String>,
    #[arg(long)]
    /// Optional hexadecimal commit, 7 to 64 ASCII characters.
    commit: Option<String>,
    /// Required for review; omit for correction.
    #[arg(long, value_enum, required_if_eq("kind", "review"))]
    verdict: Option<crate::runtime::collectors::Verdict>,
    #[arg(long, value_parser = hive_input::clap::parsers::request_id)]
    /// Stable retry key (1-128 bytes, no controls); reuse with identical input.
    pub request_id: Option<String>,
}
impl Evidence {
    pub fn request(&self) -> Result<Request> {
        self.validate_options()?;
        let (context, summary) = match (&self.summary, &self.summary_file) {
            (Some(value), None) => (Context::new("evidence", "--summary"), value.clone()),
            (None, Some(path)) => {
                let context = Context::new("evidence", "--summary-file").file(path);
                (
                    context,
                    context.text(crate::runtime::collectors::SUMMARY_BYTES, false)?,
                )
            }
            _ => {
                return Err(Context::new("evidence", "--summary").invalid(
                    hive_input::Code::ConflictingArguments,
                    "Choose one summary source.",
                    "--summary or --summary-file",
                ));
            }
        };
        crate::runtime::collectors::Evidence::validate_summary(&summary).map_err(|error| {
            context.invalid(error.code, "Invalid public evidence.", error.expected)
        })?;
        Ok(Request::Evidence(crate::runtime::collectors::Evidence {
            kind: self.kind,
            summary,
            files: self.files.clone(),
            commit: self.commit.clone(),
            verdict: self.verdict,
        }))
    }
    pub(super) fn validate_options(&self) -> Result<()> {
        crate::runtime::collectors::Evidence::validate_options(
            self.kind,
            &self.files,
            self.commit.as_deref(),
            self.verdict,
            true,
        )
        .map(|_| ())
        .map_err(|error| {
            let Some(field) = field(&error.field) else {
                return super::Error::internal_error("Missing evidence field context.").into();
            };
            Context::new("evidence", field).invalid(
                error.code,
                "Invalid public evidence.",
                error.expected,
            )
        })
    }

    pub(super) fn run(
        &self,
        root: &std::path::Path,
        id: &str,
        request: Request,
    ) -> super::Result<super::CommandOutcome> {
        let mut outcome = super::remote_response(root, id, request)?;
        if let Some(input) = outcome.input.take() {
            let input = input.file_source(
                "evidence",
                "--summary",
                "--summary-file",
                self.summary_file.as_deref(),
            )?;
            outcome.value = input.value();
            outcome.input = Some(input);
        }
        Ok(outcome)
    }
}

/// Reject unrecognized daemon pointers; indexed file labels map to repeated --file.
pub(super) fn field(pointer: &str) -> Option<&'static str> {
    match pointer {
        "/summary" => Some("--summary"),
        "/kind" => Some("KIND"),
        "/verdict" => Some("--verdict"),
        "/commit" => Some("--commit"),
        "/files" => Some("--file"),
        _ => pointer
            .strip_prefix("/files/")
            .filter(|index| {
                index
                    .parse::<usize>()
                    .is_ok_and(|value| value.to_string() == *index)
            })
            .map(|_| "--file"),
    }
}
#[derive(clap::Args)]
pub(super) struct Collectors {
    #[command(subcommand)]
    command: Command,
    #[arg(long, global = true, value_parser = hive_input::clap::parsers::request_id)]
    /// Stable retry key (1-128 bytes, no controls); reuse with identical input.
    pub request_id: Option<String>,
}
#[derive(clap::Subcommand)]
enum Command {
    /// Inspect counts, gaps, errors and attempted items.
    Status,
    /// Discard retained evidence and gap. An in-flight external write cannot be undone.
    Discard {
        #[arg(value_parser = hive_input::clap::parsers::config_name)]
        /// Configured collector destination name.
        destination: String,
    },
    /// Allow an attempted item to adopt the current key; may create a duplicate.
    ReleaseUncertain {
        #[arg(value_parser = hive_input::clap::parsers::config_name)]
        /// Configured collector destination name.
        destination: String,
        #[arg(value_parser = hive_input::clap::parsers::text(crate::limits::CONTROL_INPUT_BYTES))]
        /// Retained attempted item ID.
        item: String,
    },
    /// Explicitly choose a new Comb UUID. Attempted items still need release/discard.
    Rebind {
        #[arg(value_parser = hive_input::clap::parsers::config_name)]
        /// Configured collector destination name.
        destination: String,
        #[arg(value_parser = hive_input::clap::parsers::uuid)]
        /// New target Comb UUID to bind to.
        comb_id: String,
    },
}
impl Collectors {
    pub fn request(&self) -> Request {
        Request::CollectorAdmin(match &self.command {
            Command::Status => return Request::Collectors,
            Command::Discard { destination } => Admin {
                action: "discard".to_owned().into(),
                destination: destination.clone().into(),
                ..Default::default()
            },
            Command::ReleaseUncertain { destination, item } => Admin {
                action: "release_uncertain".to_owned().into(),
                destination: destination.clone().into(),
                item: item.clone().into(),
                ..Default::default()
            },
            Command::Rebind {
                destination,
                comb_id,
            } => Admin {
                action: "rebind".to_owned().into(),
                destination: destination.clone().into(),
                comb_id: comb_id.clone().into(),
                ..Default::default()
            },
        })
    }
}

pub(super) fn admin_command(args: &Admin) -> Option<&'static str> {
    use crate::runtime::collectors::AdminAction;
    Some(match args.action().ok()? {
        AdminAction::Discard => "collectors discard",
        AdminAction::ReleaseUncertain => "collectors release-uncertain",
        AdminAction::Rebind => "collectors rebind",
    })
}

pub(super) fn admin_field(args: &Admin, pointer: &str) -> Option<&'static str> {
    use crate::runtime::collectors::AdminAction;
    match (args.action().ok()?, pointer) {
        (_, "/destination") => Some("DESTINATION"),
        (AdminAction::ReleaseUncertain, "/item") => Some("ITEM"),
        (AdminAction::Rebind, "/comb_id") => Some("COMB_ID"),
        _ => None,
    }
}
