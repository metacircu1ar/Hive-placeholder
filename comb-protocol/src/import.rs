//! Bounded import reports contain identifiers and progress, never source paths.
use crate::{
    Error, Result,
    limits::IMPORT_PAGES,
    values::{ImportId, Slug},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportState {
    Pending,
    Completed,
    Abandoned,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ProgressRecord")]
pub struct ImportProgress {
    pub batch_id: ImportId,
    pub state: ImportState,
    pub total: usize,
    pub completed: usize,
    pub unfinished: usize,
    pub unresolved: Option<Slug>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProgressRecord {
    batch_id: ImportId,
    state: ImportState,
    total: usize,
    completed: usize,
    unfinished: usize,
    #[serde(deserialize_with = "required")]
    unresolved: Option<Slug>,
}
impl TryFrom<ProgressRecord> for ImportProgress {
    type Error = Error;
    fn try_from(raw: ProgressRecord) -> Result<Self> {
        let progress = Self {
            batch_id: raw.batch_id,
            state: raw.state,
            total: raw.total,
            completed: raw.completed,
            unfinished: raw.unfinished,
            unresolved: raw.unresolved,
        };
        progress.validate()?;
        Ok(progress)
    }
}
impl ImportProgress {
    pub fn validate(&self) -> Result<()> {
        if self.total > IMPORT_PAGES
            || self.completed > self.total
            || self.unfinished != self.total - self.completed
            || (self.state == ImportState::Completed && self.unfinished != 0)
            || (self.unresolved.is_some()
                && (self.state != ImportState::Pending || self.unfinished == 0))
        {
            return Err(Error::invalid_import_progress());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ReportRecord")]
pub struct ImportReport {
    pub batch_id: ImportId,
    pub total: usize,
    pub completed: Vec<Slug>,
    pub state: ImportState,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportRecord {
    batch_id: ImportId,
    total: usize,
    completed: Vec<Slug>,
    state: ImportState,
}
impl TryFrom<ReportRecord> for ImportReport {
    type Error = Error;
    fn try_from(raw: ReportRecord) -> Result<Self> {
        if raw.total > IMPORT_PAGES
            || raw.completed.len() != raw.total
            || raw.completed.iter().collect::<BTreeSet<_>>().len() != raw.total
            || raw.state != ImportState::Completed
        {
            return Err(Error::invalid_import_progress());
        }
        Ok(Self {
            batch_id: raw.batch_id,
            total: raw.total,
            completed: raw.completed,
            state: raw.state,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportStatus {
    #[serde(deserialize_with = "required")]
    pub current: Option<ImportProgress>,
}
fn required<'de, T: Deserialize<'de>, D: serde::Deserializer<'de>>(
    input: D,
) -> std::result::Result<Option<T>, D::Error> {
    Option::deserialize(input)
}

#[cfg(test)]
mod tests;
