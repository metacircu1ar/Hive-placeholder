//! Pure read-option validation shared by CLI, dispatch and direct storage callers.
use super::{Input, ReadArgs};
use crate::runtime::limits::{
    DEFAULT_HISTORY_PAGE_ROWS, MAX_OUTPUT_BYTES, MAX_PAGE_ROWS, MIN_OUTPUT_BYTES, MIN_PAGE_ROWS,
};
use hive_input::Code;
use std::fmt;

#[derive(Debug)]
pub(crate) struct InvalidPage {
    pub code: Code,
    pub field: &'static str,
    pub expected: &'static str,
}
impl fmt::Display for InvalidPage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.expected)
    }
}
impl std::error::Error for InvalidPage {}
impl From<InvalidPage> for crate::error::Error {
    fn from(error: InvalidPage) -> Self {
        crate::runtime::invalid_request().with_source(error)
    }
}

impl InvalidPage {
    /// Only the operation boundary can supply an accurate complete example.
    pub(crate) fn for_request(self, request: &super::Request) -> crate::error::Error {
        use super::Request;
        let example = match request {
            Request::Agents(args) if args.stalled_requested() => {
                r#"{"op":"agents","stalled_seconds":1800,"limit":100}"#
            }
            Request::Agents(_) => r#"{"op":"agents","after":0,"limit":100}"#,
            Request::Read(_) => r#"{"op":"read","target":"AGENT_ID"}"#,
            Request::Messages(_) => r#"{"op":"messages","target":"AGENT_ID","limit":100}"#,
            Request::Output(_) => {
                "{\"op\":\"output\",\"id\":\"00000000-0000-4000-8000-000000000001\",\"offset\":\
                0,\"limit\":65536}"
            }
            _ => return crate::error::Error::internal_error("Missing input-error context."),
        };
        match hive_input::InputError::new(
            self.code,
            "Invalid pagination arguments.",
            self.field,
            self.expected,
            vec![example.into()],
        ) {
            Ok(input) => crate::error::Error::from(input).with_source(self),
            Err(cause) => crate::error::Error::internal_error("Invalid input-error metadata.")
                .with_source(cause),
        }
    }
}

pub(crate) fn rows(value: u64) -> Result<u64, InvalidPage> {
    bound(
        value,
        MIN_PAGE_ROWS,
        MAX_PAGE_ROWS,
        "an integer from 1 to 200 rows",
    )
}

pub(crate) fn bytes(value: u64) -> Result<u64, InvalidPage> {
    bound(
        value,
        MIN_OUTPUT_BYTES,
        MAX_OUTPUT_BYTES,
        "an integer from 4 to 65536 bytes",
    )
}

fn bound(value: u64, min: u64, max: u64, expected: &'static str) -> Result<u64, InvalidPage> {
    if (min..=max).contains(&value) {
        Ok(value)
    } else {
        Err(InvalidPage {
            code: Code::OutOfRange,
            field: "/limit",
            expected,
        })
    }
}

pub(crate) fn number(
    value: &Input<u64>,
    default: u64,
    field: &'static str,
) -> Result<u64, InvalidPage> {
    match value {
        Input::Missing => Ok(default),
        Input::Valid(value) => Ok(*value),
        Input::Invalid(_) => Err(InvalidPage {
            code: Code::InvalidValue,
            field,
            expected: "an unsigned integer",
        }),
    }
}

#[derive(Debug)]
pub(crate) struct ReadPage {
    pub after: u64,
    pub limit: u64,
}
impl ReadArgs {
    pub(crate) fn page(&self) -> Result<ReadPage, InvalidPage> {
        let target = self.target.as_str().ok_or(InvalidPage {
            code: Code::InvalidValue,
            field: "/target",
            expected: "an agent, artifact:ID or join:ID target",
        })?;
        if target.starts_with("join:") {
            for (field, supplied) in [("/after", &self.after), ("/limit", &self.limit)] {
                if !supplied.is_missing() {
                    return Err(InvalidPage {
                        code: Code::ConflictingArguments,
                        field,
                        expected: "omit pagination for a whole join snapshot",
                    });
                }
            }
            return Ok(ReadPage {
                after: 0,
                limit: DEFAULT_HISTORY_PAGE_ROWS,
            });
        }
        let after = number(&self.after, 0, "/after")?;
        let limit = if target.starts_with("artifact:") {
            bytes(number(&self.limit, MAX_OUTPUT_BYTES, "/limit")?)?
        } else {
            rows(number(&self.limit, DEFAULT_HISTORY_PAGE_ROWS, "/limit")?)?
        };
        Ok(ReadPage { after, limit })
    }
}
