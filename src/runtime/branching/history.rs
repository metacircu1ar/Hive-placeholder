//! Verify the actual inherited dialogue at each native Claude branch.
use crate::{
    error::{Error, Result},
    state,
};
use serde_json::Value;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};
const MAX_PROJECTS: usize = 4096;
const MAX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RECORDS: usize = 100_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::runtime) struct Dialogue {
    pub id: String,
    row: Value,
}
pub(in crate::runtime) fn refused() -> Error {
    Error::unsupported_operation(
        "This Claude branch could not verify its copied conversation history; the \
            branch was refused.",
    )
}
fn read(home: &Path, session: &str) -> Result<Vec<Dialogue>> {
    read_optional(home, session)?
        .filter(|rows| !rows.is_empty())
        .ok_or_else(refused)
}
fn read_optional(home: &Path, session: &str) -> Result<Option<Vec<Dialogue>>> {
    uuid::Uuid::parse_str(session).map_err(|cause| refused().with_source(cause))?;
    let projects = home.join("projects");
    state::check_path(&projects, true)?;
    let mut found = None;
    for (count, entry) in std::fs::read_dir(&projects)
        .map_err(Error::state_io_from)?
        .enumerate()
    {
        if count >= MAX_PROJECTS {
            return Err(refused());
        }
        let entry = entry.map_err(Error::state_io_from)?;
        if !entry.file_type().map_err(Error::state_io_from)?.is_dir() {
            continue;
        }
        let path = entry.path().join(format!("{session}.jsonl"));
        match std::fs::symlink_metadata(&path) {
            Ok(_) => {
                state::check_path(&entry.path(), true)?;
                if found.replace(path).is_some() {
                    return Err(refused());
                }
            }
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
            Err(cause) => return Err(Error::state_io_from(cause)),
        }
    }
    let Some(path) = found else {
        return Ok(None);
    };
    let identity = crate::runtime::provider_binary_identity(&path)?;
    let bytes = crate::os::read_capped(state::private_reader(&path)?, MAX_BYTES + 1)
        .map_err(Error::state_io_from)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(refused());
    }
    // An append in progress is not evidence yet; the branch gate retries within
    // its fixed deadline without releasing tools, output or the source hold.
    if !identity.unchanged() || !bytes.ends_with(b"\n") {
        return Ok(None);
    }
    decode(&bytes).map(Some)
}
fn decode(bytes: &[u8]) -> Result<Vec<Dialogue>> {
    let mut rows = Vec::new();
    let mut ids = HashSet::new();
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let value: Value =
            serde_json::from_slice(line).map_err(|cause| refused().with_source(cause))?;
        if !matches!(
            value.get("type").and_then(Value::as_str),
            Some("user" | "assistant")
        ) {
            continue;
        }
        if rows.len() >= MAX_RECORDS || value.get("isSidechain") == Some(&Value::Bool(true)) {
            return Err(refused());
        }
        let id = value
            .get("uuid")
            .and_then(Value::as_str)
            .ok_or_else(refused)?;
        uuid::Uuid::parse_str(id).map_err(|cause| refused().with_source(cause))?;
        let message = value
            .get("message")
            .filter(|v| v.is_object())
            .ok_or_else(refused)?;
        if message.get("role") != value.get("type")
            || !message
                .get("content")
                .is_some_and(|c| c.is_string() || c.is_array())
        {
            return Err(refused());
        }
        let parent = value.get("parentUuid").ok_or_else(refused)?;
        if !parent.is_null() {
            uuid::Uuid::parse_str(parent.as_str().ok_or_else(refused)?)
                .map_err(|cause| refused().with_source(cause))?;
        }
        // Only one rooted main dialogue chain establishes inherited context. A
        // physical copy of disconnected rows does not establish inherited context.
        let expected_parent = rows.last().map(|row: &Dialogue| row.id.as_str());
        if parent.as_str() != expected_parent || (rows.is_empty() && !parent.is_null()) {
            return Err(refused());
        }
        if !ids.insert(id.to_owned()) {
            return Err(refused());
        }
        rows.push(Dialogue {
            id: id.into(),
            row: serde_json::json!({"uuid":id,
            "parentUuid":parent,
            "type":value["type"],
            "message":message}),
        });
    }
    Ok(rows)
}
pub(in crate::runtime) struct Snapshot {
    home: PathBuf,
    source: String,
    child: String,
    rows: Vec<Dialogue>,
}
impl Snapshot {
    pub(in crate::runtime) fn capture(
        home: &Path,
        source: &str,
        child: &str,
        cutoff: &str,
    ) -> Result<Self> {
        if source == child {
            return Err(refused());
        }
        let rows = read(home, source)?;
        if !rows
            .iter()
            .any(|row| row.id == cutoff && row.row["type"] == "user")
        {
            return Err(refused());
        }
        Ok(Self {
            home: home.into(),
            source: source.into(),
            child: child.into(),
            rows,
        })
    }
    pub(in crate::runtime) fn verify(&self, input: &str) -> Result<bool> {
        if read(&self.home, &self.source)? != self.rows {
            return Err(refused());
        }
        let Some(child) = read_optional(&self.home, &self.child)? else {
            return Ok(false);
        };
        if child.len() <= self.rows.len() {
            return Ok(false);
        }
        extends(&self.rows, &child)?;
        let first = &child[self.rows.len()];
        if first.id != input || first.row["type"] != "user" {
            return Err(refused());
        }
        Ok(true)
    }
}
fn extends(before: &[Dialogue], after: &[Dialogue]) -> Result<()> {
    if after.len() > before.len() && after.starts_with(before) {
        Ok(())
    } else {
        Err(refused())
    }
}
#[cfg(test)]
mod tests;
