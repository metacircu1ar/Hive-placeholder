//! Checked SQLite numeric decoding and binding error contexts.
use crate::error::Error;
use rusqlite::{Result, Row};
pub(super) fn numeric_range() -> Error {
    Error::database_error("Runtime numeric value is out of range.")
}
pub(super) fn numeric(cause: std::num::TryFromIntError) -> Error {
    numeric_range().with_source(cause)
}

pub(super) fn nonnegative(row: &Row<'_>, index: usize) -> Result<i64> {
    let value: i64 = row.get(index)?;
    if value < 0 {
        Err(rusqlite::Error::IntegralValueOutOfRange(index, value))
    } else {
        Ok(value)
    }
}

pub(super) fn unsigned(row: &Row<'_>, index: usize) -> Result<u64> {
    let value: i64 = row.get(index)?;
    // This SQLite variant retains the column and bad value but has no source slot.
    crate::convert::unsigned_u64(value)
        .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}

pub(super) fn optional_unsigned(row: &Row<'_>, index: usize) -> Result<Option<u64>> {
    // Preserve SQLite's column/value error shape; it cannot carry a nested cause.
    row.get::<_, Option<i64>>(index)?
        .map(|value| {
            crate::convert::unsigned_u64(value)
                .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
        })
        .transpose()
}

pub(super) fn count(row: &Row<'_>, index: usize) -> Result<usize> {
    let value: i64 = row.get(index)?;
    // Preserve SQLite's column/value error shape; it cannot carry a nested cause.
    crate::convert::count_usize(value)
        .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}

/// Bind an unsigned timestamp or count using the runtime numeric error context.
pub(super) fn stamp(value: u64) -> crate::error::Result<i64> {
    crate::convert::sqlite_i64(value).map_err(numeric)
}

pub(super) fn timestamp(row: &Row<'_>, index: usize) -> rusqlite::Result<u64> {
    checked_timestamp(row.get(index)?, index)
}
pub(super) fn optional_timestamp(row: &Row<'_>, index: usize) -> rusqlite::Result<Option<u64>> {
    row.get::<_, Option<i64>>(index)?
        .map(|v| checked_timestamp(v, index))
        .transpose()
}
fn checked_timestamp(value: i64, index: usize) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(e),
        )
    })
}

pub(super) fn now() -> crate::error::Result<i64> {
    stamp(crate::state::now())
}
