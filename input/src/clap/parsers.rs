//! Owned clap-compatible values backed by the same pure server-side validators.
use crate::validate::{self, Violation};
use std::ops::RangeInclusive;

pub fn text(max_bytes: usize) -> impl Fn(&str) -> Result<String, Violation> + Clone {
    move |value| {
        validate::text_bytes(value, max_bytes)?;
        Ok(value.into())
    }
}

pub fn config_name(value: &str) -> Result<String, Violation> {
    validate::config_name(value)?;
    Ok(value.into())
}

pub fn request_id(value: &str) -> Result<String, Violation> {
    validate::request_id(value)?;
    Ok(value.into())
}

pub fn model(value: &str) -> Result<String, Violation> {
    validate::model(value)?;
    Ok(value.into())
}

pub fn agent_name(value: &str) -> Result<String, Violation> {
    validate::agent_name(value)?;
    Ok(value.into())
}

pub fn effort(value: &str) -> Result<String, Violation> {
    validate::effort(value)?;
    Ok(value.into())
}

pub fn run_name(max_bytes: usize) -> impl Fn(&str) -> Result<String, Violation> + Clone {
    move |value| {
        validate::run_name(value, max_bytes)?;
        Ok(value.into())
    }
}

pub fn duration(bounds: RangeInclusive<u64>) -> impl Fn(&str) -> Result<u64, Violation> + Clone {
    move |value| validate::duration(value, bounds.clone())
}

pub fn unsigned(bounds: RangeInclusive<u64>) -> impl Fn(&str) -> Result<u64, Violation> + Clone {
    move |value| validate::unsigned(value, bounds.clone())
}

/// Decimal platform-sized count with explicit policy bounds.
pub fn count(bounds: RangeInclusive<usize>) -> impl Fn(&str) -> Result<usize, Violation> + Clone {
    move |value| validate::count(value, bounds.clone())
}

/// Validate an identifier without changing its accepted spelling.
pub fn uuid(value: &str) -> Result<String, Violation> {
    validate::uuid(value)?;
    Ok(value.into())
}
