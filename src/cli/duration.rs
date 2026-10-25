//! Parse human duration suffixes with the shared bounded validator.
pub(super) fn parse(raw: &str) -> Result<u64, hive_input::validate::Violation> {
    hive_input::validate::duration(raw, 1..=i64::MAX as u64)
}

pub(super) fn delay(raw: &str) -> Result<u64, hive_input::validate::Violation> {
    hive_input::validate::duration(raw, 1..=crate::runtime::limits::MAX_DELAY)
}

#[cfg(test)]
mod tests;
