//! Insertion-only YAML changes, with a semantic check after every text patch.
use super::{Result, invalid};
use serde_yaml_ng::{Mapping, Value};
use std::{
    fs::{File, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};
use yaml_rust2::scanner::{Scanner, TokenType};
pub(super) const LIMIT: u64 = 1024 * 1024;

pub(super) fn read_text(path: &Path) -> Result<Option<String>> {
    match read_required_text(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.io_kind() == Some(std::io::ErrorKind::NotFound) => Ok(None),
        Err(error) => Err(error),
    }
}
pub(super) fn read_required_text(path: &Path) -> Result<String> {
    read_file_text(path).map(|(_, text)| text)
}
pub(super) fn read_file_text(path: &Path) -> Result<(File, String)> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(read_error)?;
    let m = file.metadata().map_err(read_error)?;
    if !m.is_file()
        || m.nlink() != 1
        || m.uid() != rustix::process::getuid().as_raw()
        || m.mode() & 0o022 != 0
        || m.len() > LIMIT
    {
        return Err(invalid(
            "Setup input must be a bounded owned regular file, not writable by other users.",
        ));
    }
    let mut text = String::new();
    (&file)
        .take(LIMIT + 1)
        .read_to_string(&mut text)
        .map_err(read_error)?;
    if text.len() as u64 > LIMIT {
        return Err(invalid("Setup input exceeds 1 MiB."));
    }
    Ok((file, text))
}
fn read_error(cause: std::io::Error) -> crate::error::Error {
    crate::error::Error::state_io("Cannot read setup input.").with_source(cause)
}
pub(super) fn strict_yaml(text: &str) -> Result<Value> {
    let mut scanner = Scanner::new(text.chars());
    let forbidden = scanner.by_ref().any(|t| {
        matches!(
            t.1,
            TokenType::Anchor(_) | TokenType::Alias(_) | TokenType::Tag(_, _)
        )
    });
    if let Some(cause) = scanner.get_error() {
        return Err(invalid("Invalid setup YAML.").with_source(cause));
    }
    if forbidden {
        return Err(invalid(
            "Setup YAML must not contain anchors, aliases or tags.",
        ));
    }
    let value: Value = serde_yaml_ng::from_str(text)
        .map_err(|cause| invalid("Invalid setup YAML.").with_source(cause))?;
    if !safe(&value) {
        return Err(invalid(
            "Setup YAML requires string keys and no merge keys.",
        ));
    }
    Ok(value)
}
pub(super) fn yaml(value: &Value) -> Result<String> {
    serde_yaml_ng::to_string(value)
        .map_err(|e| invalid("Cannot serialize setup YAML.").with_source(e))
}
fn conflict(path: &str, desired: &Value) -> crate::error::Error {
    invalid(format!(
        "Setup will not overwrite {path} or rewrite a complex YAML map. Add the \
            following values manually, then rerun:\n{}",
        crate::observability::optional(serde_yaml_ng::to_string(desired), "setup_conflict_example")
            .unwrap_or_default()
    ))
}
pub(super) fn merge(old: &Value, wanted: &Value) -> Result<Value> {
    if old == wanted {
        return Ok(old.clone());
    }
    let (Some(old), Some(wanted)) = (old.as_mapping(), wanted.as_mapping()) else {
        return Err(conflict("existing value", wanted));
    };
    let mut result = old.clone();
    for (key, value) in wanted {
        result.insert(
            key.clone(),
            match old.get(key) {
                Some(old) => merge(old, value).map_err(|cause| {
                    conflict(
                        key.as_str().unwrap_or("key"),
                        &Value::Mapping(wanted.clone()),
                    )
                    .with_source(cause)
                })?,
                None => value.clone(),
            },
        );
    }
    Ok(Value::Mapping(result))
}
pub(super) fn patch(text: &str, wanted: &Value) -> Result<String> {
    if text.is_empty() {
        return yaml(wanted);
    }
    let before = strict_yaml(text)?;
    let expected = merge(&before, wanted)?;
    if before == expected {
        return Ok(text.into());
    }
    let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_owned).collect();
    if let Some(last) = lines.last_mut()
        && !last.ends_with('\n')
    {
        last.push('\n');
    }
    insert(&mut lines, &before, wanted, 0)?;
    let result = lines.concat();
    if strict_yaml(&result)? != expected {
        return Err(conflict("document structure", wanted));
    }
    Ok(result)
}
fn insert(lines: &mut Vec<String>, before: &Value, wanted: &Value, indent: usize) -> Result<()> {
    let old = before
        .as_mapping()
        .ok_or_else(|| conflict("mapping", wanted))?;
    let desired = wanted
        .as_mapping()
        .ok_or_else(|| conflict("mapping", wanted))?;
    for (key, value) in desired {
        let name = key.as_str().ok_or_else(|| conflict("key", wanted))?;
        match old.get(key) {
            Some(existing) if merge(existing, value)? == *existing => (),
            Some(existing) => {
                let needle = format!("{}{name}:", " ".repeat(indent));
                let start = lines
                    .iter()
                    .position(|line| {
                        line.starts_with(&needle)
                            && line[needle.len()..].trim().is_empty_or_comment()
                    })
                    .ok_or_else(|| conflict(name, wanted))?;
                let end = (start + 1..lines.len())
                    .find(|&i| {
                        let trimmed = lines[i].trim();
                        !trimmed.is_empty()
                            && !trimmed.starts_with('#')
                            && lines[i].len() - lines[i].trim_start().len() <= indent
                    })
                    .unwrap_or(lines.len());
                let child_indent = lines[start + 1..end]
                    .iter()
                    .find_map(|line| {
                        let s = line.trim_start();
                        (!s.trim().is_empty() && !s.starts_with('#'))
                            .then_some(line.len() - s.len())
                    })
                    .ok_or_else(|| conflict(name, wanted))?;
                let mut child = lines[start + 1..end].to_vec();
                insert(&mut child, existing, value, child_indent)?;
                lines.splice(start + 1..end, child);
            }
            None => {
                let mut one = Mapping::new();
                one.insert(key.clone(), value.clone());
                let text = yaml(&Value::Mapping(one))?;
                lines.extend(
                    text.lines()
                        .map(|line| format!("{}{line}\n", " ".repeat(indent))),
                );
            }
        }
    }
    Ok(())
}
trait Comment {
    fn is_empty_or_comment(&self) -> bool;
}
impl Comment for str {
    fn is_empty_or_comment(&self) -> bool {
        self.is_empty() || self.starts_with('#')
    }
}

fn safe(value: &Value) -> bool {
    match value {
        Value::Mapping(m) => m
            .iter()
            .all(|(k, v)| k.as_str().is_some_and(|k| k != "<<") && safe(v)),
        Value::Sequence(s) => s.iter().all(safe),
        Value::Tagged(_) => false,
        _ => true,
    }
}
