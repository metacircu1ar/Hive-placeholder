//! Trusted diagnostic context for the human approval operations.
use super::Input;
use crate::error::{Error, Result};
use hive_input::Code;

#[derive(Clone, Copy)]
pub(crate) enum ApprovalInput {
    List,
    Show,
    Respond,
}
impl ApprovalInput {
    pub(crate) fn error(self, code: Code, field: &'static str, expected: &'static str) -> Error {
        let example = match self {
            Self::List => r#"{"op":"approvals","limit":100}"#,
            Self::Show => {
                r#"{"op":"approval","approval_id":"00000000-0000-4000-8000-000000000001"}"#
            }
            Self::Respond => {
                "{\"op\":\"respond\",\"approval_id\":\"00000000-0000-4000-8000-000000000001\",\
                \"action\":\"accept\"}"
            }
        };
        hive_input::InputError::new(
            code,
            "Invalid approval arguments.",
            field,
            expected,
            vec![example.into()],
        )
        .map_or_else(
            |cause| Error::internal_error("Invalid approval input metadata.").with_source(cause),
            Error::from,
        )
    }
    pub(crate) fn id(self, value: &Input<String>) -> Result<&str> {
        match value {
            Input::Missing => {
                Err(self.error(Code::MissingArgument, "/approval_id", "an approval UUID"))
            }
            Input::Valid(value) if hive_input::validate::uuid(value).is_ok() => Ok(value),
            _ => Err(self.error(Code::InvalidUuid, "/approval_id", "an approval UUID")),
        }
    }
}
