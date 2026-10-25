//! Every parameter must have exactly one named binding before a raw query runs.
use crate::search::Filter;
use rusqlite::{Error as SqlError, Result, Statement, ToSql};

pub(in crate::index) fn bind(
    statement: &mut Statement<'_>,
    filter: Filter<'_>,
    extra: &[(&str, &dyn ToSql)],
) -> Result<()> {
    let filters: [(&str, &dyn ToSql); 5] = [
        (":kind", &filter.kind),
        (":tag", &filter.tag),
        (":prefix", &filter.prefix),
        (":after", &filter.after),
        (":before", &filter.before),
    ];
    let bindings = filters.iter().chain(extra);
    let supplied = filters.len() + extra.len();
    let expected = statement.parameter_count();
    if supplied != expected {
        return Err(SqlError::InvalidParameterCount(supplied, expected));
    }
    // Equal counts plus complete name coverage prove a bijection: an unknown,
    // duplicate or omitted binding cannot leave a parameter silently NULL.
    for index in 1..=expected {
        let name = statement
            .parameter_name(index)
            .ok_or_else(|| SqlError::InvalidParameterName(format!("?{index}")))?;
        if !bindings.clone().any(|(key, _)| *key == name) {
            return Err(SqlError::InvalidParameterName(name.to_owned()));
        }
    }
    for index in 1..=expected {
        let name = statement
            .parameter_name(index)
            .ok_or_else(|| SqlError::InvalidParameterName(format!("?{index}")))?;
        let (_, value) = bindings
            .clone()
            .find(|(key, _)| *key == name)
            .ok_or_else(|| SqlError::InvalidParameterName(name.to_owned()))?;
        statement.raw_bind_parameter(index, *value)?;
    }
    Ok(())
}
