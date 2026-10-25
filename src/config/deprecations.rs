//! Explicit released-key deprecations. The development table is deliberately empty.
use crate::error::{Error, Result};
use serde::Serialize;
use serde_yaml_ng::Value;

/// A compiled key path, with `*` allowed for a mapping of named roles/profiles.
/// Replacement renames only the final key; it never guesses a value conversion.
pub(crate) struct Entry {
    pub path: &'static [&'static str],
    pub replacement: Option<&'static str>,
    pub since: &'static str,
    pub remove_in_major: u64,
}
pub(crate) const TABLE: &[Entry] = &[];

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct Notice {
    pub key: String,
    pub replacement: Option<&'static str>,
    pub since: &'static str,
    pub remove_in_major: u64,
    pub occurrences: usize,
    pub fixable: bool,
}
fn notice(entry: &Entry, occurrences: usize) -> Notice {
    Notice {
        key: entry.path.join("."),
        replacement: entry.replacement,
        since: entry.since,
        remove_in_major: entry.remove_in_major,
        occurrences,
        fixable: entry.replacement.is_some(),
    }
}
pub(crate) fn inspect(value: &Value, entries: &[Entry]) -> Vec<Notice> {
    entries
        .iter()
        .filter_map(|entry| {
            let count = occurrences(value, entry.path);
            (count > 0).then(|| notice(entry, count))
        })
        .collect()
}
fn occurrences(value: &Value, path: &[&str]) -> usize {
    let Some((key, rest)) = path.split_first() else {
        return 1;
    };
    let Some(mapping) = value.as_mapping() else {
        return 0;
    };
    if *key == "*" {
        mapping.values().map(|v| occurrences(v, rest)).sum()
    } else {
        mapping
            .get(Value::String((*key).into()))
            .map_or(0, |v| occurrences(v, rest))
    }
}
pub(crate) fn warn(notices: &[Notice]) {
    if !notices.is_empty() {
        // Details are static registry metadata in config check/status, never values.
        tracing::warn!(name: "config.deprecated",
             code = "deprecated_config",
             count = notices.len(),
             decision = "run_config_check");
    }
}

pub(crate) struct Correction {
    pub text: String,
    pub applied: Vec<Notice>,
    pub remaining: Vec<Notice>,
}
pub(crate) fn correct(text: &str, entries: &[Entry], path: &std::path::Path) -> Result<Correction> {
    let mut value = super::Config::yaml(text)?;
    let mut applied = Vec::new();
    for entry in entries {
        if let Some(replacement) = entry.replacement {
            let count = rename(&mut value, entry.path, replacement)?;
            if count > 0 {
                applied.push(notice(entry, count));
            }
        }
    }
    let remaining = inspect(&value, entries);
    let text = if applied.is_empty() {
        text.into()
    } else {
        serde_yaml_ng::to_string(&value).map_err(|cause| {
            Error::config_invalid("Cannot encode corrected configuration.").with_source(cause)
        })?
    };
    // A table entry never bypasses the current configuration contract.
    super::Config::parse_at(&text, path)?;
    Ok(Correction {
        text,
        applied,
        remaining,
    })
}
fn rename(value: &mut Value, path: &[&str], replacement: &str) -> Result<usize> {
    let Some((key, rest)) = path.split_first() else {
        return Err(Error::internal_error(
            "Empty configuration deprecation path.",
        ));
    };
    let Some(mapping) = value.as_mapping_mut() else {
        return Ok(0);
    };
    if *key == "*" {
        return mapping
            .values_mut()
            .try_fold(0, |count, v| Ok(count + rename(v, rest, replacement)?));
    }
    let key = Value::String((*key).into());
    if !rest.is_empty() {
        return mapping
            .get_mut(&key)
            .map_or(Ok(0), |v| rename(v, rest, replacement));
    }
    if !mapping.contains_key(&key) {
        return Ok(0);
    }
    let replacement = Value::String(replacement.into());
    if mapping.contains_key(&replacement) {
        return Err(Error::config_invalid(
            "A deprecated configuration key conflicts with its replacement; resolve it manually.",
        ));
    }
    let value = mapping
        .remove(&key)
        .ok_or_else(|| Error::internal_error("Configuration deprecation key disappeared."))?;
    mapping.insert(replacement, value);
    Ok(1)
}

#[cfg(test)]
mod tests;
