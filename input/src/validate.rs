//! Pure syntax and range validators shared by frontends and servers.
use crate::Code;
use std::{fmt, ops::RangeInclusive};

pub const REQUEST_ID_BYTES: usize = 128;
pub const MODEL_BYTES: usize = 128;
pub const CONFIG_NAME_BYTES: usize = 64;
pub const AGENT_NAME_BYTES: usize = 64;
pub const EFFORTS: &[&str] = &[
    "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
];

/// A safe description of the broken rule. Never contains the rejected value.
/// Only these validators construct violations; custom parser errors are untrusted.
///
/// ```compile_fail
/// let error = hive_input::validate::Violation {
///     code: hive_input::Code::InvalidValue,
///     expected: "private parser text".into(),
/// };
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Violation {
    code: Code,
    expected: String,
}
impl Violation {
    pub fn code(&self) -> Code {
        self.code
    }

    pub fn expected(&self) -> &str {
        &self.expected
    }
}
impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.expected)
    }
}
impl std::error::Error for Violation {}

fn violation(code: Code, expected: impl Into<String>) -> Violation {
    Violation {
        code,
        expected: expected.into(),
    }
}

fn bounded_text(value: &str, max_bytes: usize, code: Code) -> Result<(), Violation> {
    if value.is_empty() || value.len() > max_bytes || value.chars().any(char::is_control) {
        Err(violation(
            code,
            format!("1..{max_bytes} UTF-8 bytes without control characters"),
        ))
    } else {
        Ok(())
    }
}

/// Content can be empty when the operation gives it an explicit meaning.
pub fn byte_count(length: usize, max_bytes: usize) -> Result<(), Violation> {
    if length > max_bytes {
        Err(violation(
            Code::OutOfRange,
            format!("at most {max_bytes} bytes"),
        ))
    } else {
        Ok(())
    }
}

/// Shape only: the caller checks existence and resolves symlinks separately.
pub fn absolute_path(path: &std::path::Path) -> Result<(), Violation> {
    if !path.is_absolute() || path.to_str().is_none_or(|text| text.contains('\0')) {
        Err(violation(
            Code::InvalidPath,
            "an absolute UTF-8 directory path without NUL bytes",
        ))
    } else {
        Ok(())
    }
}

/// Opaque identifiers and content retain their exact UTF-8 spelling.
pub fn text_bytes(value: &str, max_bytes: usize) -> Result<(), Violation> {
    if value.is_empty() || value.len() > max_bytes {
        Err(violation(
            Code::OutOfRange,
            format!("a string within 1..{max_bytes} UTF-8 bytes"),
        ))
    } else {
        Ok(())
    }
}

pub fn config_name(value: &str) -> Result<(), Violation> {
    if value.is_empty()
        || value.len() > CONFIG_NAME_BYTES
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        Err(violation(
            Code::InvalidValue,
            "1..64 ASCII letters, digits, '-' or '_'",
        ))
    } else {
        Ok(())
    }
}

pub fn request_id(value: &str) -> Result<(), Violation> {
    bounded_text(value, REQUEST_ID_BYTES, Code::InvalidRequestId)
}

pub fn run_name(value: &str, max_bytes: usize) -> Result<(), Violation> {
    bounded_text(value, max_bytes, Code::InvalidRunName)
}

/// An agent's display label: never an identifier, so it need not be unique.
pub fn agent_name(value: &str) -> Result<(), Violation> {
    if value.is_empty()
        || value.len() > AGENT_NAME_BYTES
        || value.chars().any(char::is_control)
        || value.trim() != value
    {
        Err(violation(
            Code::InvalidValue,
            "1..64 UTF-8 bytes without control characters or surrounding spaces",
        ))
    } else {
        Ok(())
    }
}

/// Provider catalogs additionally decide whether a syntactically valid model exists.
pub fn model(value: &str) -> Result<(), Violation> {
    if value.is_empty()
        || value.len() > MODEL_BYTES
        || value.starts_with('-')
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:@/[]".contains(&b))
    {
        return Err(violation(
            Code::InvalidModel,
            "1..128 ASCII letters, digits or -_.:@/[]; no leading '-'",
        ));
    }
    Ok(())
}

pub fn effort(value: &str) -> Result<(), Violation> {
    if EFFORTS.contains(&value) {
        Ok(())
    } else {
        Err(violation(Code::InvalidEffort, EFFORTS.join(", ")))
    }
}

pub fn uuid(value: &str) -> Result<uuid::Uuid, Violation> {
    uuid::Uuid::parse_str(value).map_err(|_| violation(Code::InvalidUuid, "a UUID"))
}

/// Parse a platform-sized count without clamping or accepting signs/whitespace.
pub fn count(value: &str, bounds: RangeInclusive<usize>) -> Result<usize, Violation> {
    let expected = || {
        format!(
            "a decimal integer within {}..{}",
            bounds.start(),
            bounds.end()
        )
    };
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(violation(Code::OutOfRange, expected()));
    }
    value
        .parse::<usize>()
        .ok()
        .filter(|number| bounds.contains(number))
        .ok_or_else(|| violation(Code::OutOfRange, expected()))
}

/// Parse a decimal integer, rejecting signs, whitespace, fractions and overflow.
pub fn unsigned(value: &str, bounds: RangeInclusive<u64>) -> Result<u64, Violation> {
    let parsed = decimal(value)
        .ok_or_else(|| violation(Code::InvalidValue, "an unsigned decimal integer"))?;
    integer(parsed, bounds)
}

/// Check an already decoded integer against the same bounds as a CLI number.
pub fn integer(value: u64, bounds: RangeInclusive<u64>) -> Result<u64, Violation> {
    if bounds.contains(&value) {
        Ok(value)
    } else {
        Err(violation(
            Code::OutOfRange,
            format!("an integer in {}..={}", bounds.start(), bounds.end()),
        ))
    }
}

/// Parse a positive suffixed integer into seconds. Bounds are inclusive seconds.
pub fn duration(value: &str, bounds: RangeInclusive<u64>) -> Result<u64, Violation> {
    let invalid = || {
        violation(
            Code::InvalidDuration,
            format!(
                "a positive integer followed by s, m, h, d or w, within {}..={} seconds",
                bounds.start(),
                bounds.end()
            ),
        )
    };
    let (last, number) = value.as_bytes().split_last().ok_or_else(invalid)?;
    let multiplier = match last {
        b's' => 1,
        b'm' => 60,
        b'h' => 3600,
        b'd' => 86400,
        b'w' => 604_800,
        _ => return Err(invalid()),
    };
    let seconds = std::str::from_utf8(number)
        .ok()
        .and_then(decimal)
        .and_then(|number| number.checked_mul(multiplier))
        .filter(|seconds| *seconds > 0 && bounds.contains(seconds))
        .ok_or_else(invalid)?;
    Ok(seconds)
}

fn decimal(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}
