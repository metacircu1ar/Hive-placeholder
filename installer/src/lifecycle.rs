//! Publication hooks keep service locks alive for the entire installation transaction.
use crate::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Install,
    Uninstall,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub operation: Operation,
    pub removed_links: BTreeMap<PathBuf, PathBuf>,
    pub prefix: PathBuf,
    pub manager: PathBuf,
    pub previous: Option<PathBuf>,
    pub next: PathBuf,
    pub added_links: BTreeMap<PathBuf, PathBuf>,
    pub components: Vec<String>,
}
pub trait Lifecycle {
    fn preview_removal(&self, prefix: &Path, components: &[String]) -> Result<()>;
    fn recover(&mut self, prefix: &Path, manager: &Path) -> Result<()>;
    fn prepare(&mut self, change: &Change) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
    fn finish(&mut self, published: bool) -> Result<()>;
}
#[cfg(test)]
pub struct Inert;
#[cfg(test)]
impl Lifecycle for Inert {
    fn preview_removal(&self, _: &Path, _: &[String]) -> Result<()> {
        Ok(())
    }
    fn recover(&mut self, _: &Path, _: &Path) -> Result<()> {
        Ok(())
    }
    fn prepare(&mut self, _: &Change) -> Result<()> {
        Ok(())
    }
    fn stop(&mut self) -> Result<()> {
        Ok(())
    }
    fn finish(&mut self, _: bool) -> Result<()> {
        Ok(())
    }
}
