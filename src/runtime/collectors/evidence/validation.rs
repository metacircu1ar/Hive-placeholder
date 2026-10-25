//! Pure field rules: diagnostics contain trusted descriptions, never supplied values.
use super::{CollectionSource, Error, Evidence, EvidenceKind, SUMMARY_BYTES, Verdict, private};
use hive_input::Code;

#[derive(Debug)]
pub(crate) struct InvalidEvidence {
    pub code: Code,
    pub field: String,
    pub expected: &'static str,
}
impl InvalidEvidence {
    fn new(code: Code, field: impl Into<String>, expected: &'static str) -> Self {
        Self {
            code,
            field: field.into(),
            expected,
        }
    }
}
impl From<InvalidEvidence> for Error {
    fn from(error: InvalidEvidence) -> Self {
        hive_input::InputError::new(
            error.code,
            "Invalid public evidence.",
            error.field,
            error.expected,
            vec![
                "{\"op\":\"evidence\",\"kind\":\"review\",\"summary\":\"Public review: no \
                issues found.\",\"verdict\":\"no_findings\"}"
                    .into(),
            ],
        )
        .map_or_else(
            |cause| Self::internal_error("Invalid evidence error metadata.").with_source(cause),
            Self::from,
        )
    }
}
impl Evidence {
    pub(crate) fn validate_summary(summary: &str) -> Result<(), InvalidEvidence> {
        if summary.len() > SUMMARY_BYTES {
            return Err(InvalidEvidence::new(
                Code::OutOfRange,
                "/summary",
                "at most 8192 UTF-8 bytes",
            ));
        }
        if summary.trim().is_empty() || summary.contains('\0') || private(summary) {
            return Err(InvalidEvidence::new(
                Code::InvalidValue,
                "/summary",
                "nonblank explicitly public text without NUL or private markers",
            ));
        }
        Ok(())
    }

    pub(crate) fn validate_options(
        kind: EvidenceKind,
        files: &[String],
        commit: Option<&str>,
        verdict: Option<Verdict>,
        human: bool,
    ) -> Result<CollectionSource, InvalidEvidence> {
        if files.len() > 64 {
            return Err(InvalidEvidence::new(
                Code::OutOfRange,
                "/files",
                "at most 64 file labels",
            ));
        }
        for (index, file) in files.iter().enumerate() {
            if file.len() > 512 {
                return Err(InvalidEvidence::new(
                    Code::OutOfRange,
                    format!("/files/{index}"),
                    "at most 512 UTF-8 bytes per file label",
                ));
            }
            if file.is_empty()
                || private(file)
                || file.starts_with('/')
                || file.starts_with('~')
                || file.contains('\\')
                || file.contains(':')
                || file.chars().any(char::is_control)
                || file.split('/').any(|part| matches!(part, ".." | "." | ""))
            {
                return Err(InvalidEvidence::new(
                    Code::InvalidValue,
                    format!("/files/{index}"),
                    "a nonempty public relative file label without empty/dot components, \
                        controls, backslashes or colons",
                ));
            }
        }
        if commit.is_some_and(|value| {
            !(7..=64).contains(&value.len()) || !value.bytes().all(|b| b.is_ascii_hexdigit())
        }) {
            return Err(InvalidEvidence::new(
                Code::InvalidValue,
                "/commit",
                "7 to 64 hexadecimal ASCII characters",
            ));
        }
        match kind {
            EvidenceKind::Review if verdict.is_none() => Err(InvalidEvidence::new(
                Code::MissingArgument,
                "/verdict",
                "a verdict for review: findings or no_findings",
            )),
            EvidenceKind::Review => Ok(CollectionSource::Review),
            EvidenceKind::Correction if !human => Err(InvalidEvidence::new(
                Code::InvalidValue,
                "/kind",
                "review for hosted agents; correction is human-only",
            )),
            EvidenceKind::Correction if verdict.is_some() => Err(InvalidEvidence::new(
                Code::ConflictingArguments,
                "/verdict",
                "omit verdict for correction",
            )),
            EvidenceKind::Correction => Ok(CollectionSource::Correction),
        }
    }
}
