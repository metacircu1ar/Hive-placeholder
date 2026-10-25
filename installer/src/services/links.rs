//! Only journal-authorized links can be completed after the pointer is selected.
use super::{Journal, Operation};
use crate::{Result, publish};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(super) fn validate(journal: &Journal, old: &BTreeMap<PathBuf, PathBuf>) -> Result<()> {
    let change = &journal.change;
    if change.operation == Operation::Uninstall {
        if !change.added_links.is_empty() {
            return Err("Uninstall cannot add command links".into());
        }
        return Ok(());
    }
    let next = publish::load(
        &change.manager,
        Some(&change.next),
        &crate::manifest::native_target()?,
    )?;
    let expected = publish::links(&next.parts)?;
    let added = if change.operation == Operation::Install {
        expected
            .into_iter()
            .filter(|(name, _)| !old.contains_key(name))
            .collect()
    } else {
        BTreeMap::new()
    };
    if change.added_links != added {
        return Err("Invalid pending command ownership".into());
    }
    Ok(())
}

pub(super) fn recover(
    prefix: &Path,
    manager: &Path,
    journal: &Journal,
    previous: bool,
) -> Result<()> {
    let pointer = if previous {
        journal.change.previous.as_deref()
    } else {
        Some(journal.change.next.as_path())
    };
    let installed = publish::load(manager, pointer, &crate::manifest::native_target()?)?;
    let expected = publish::links(&installed.parts)?;
    // Repeat the pointer barrier before any command can expose this generation.
    publish::sync(manager)?;
    super::reconcile_removed(prefix, &journal.change.removed_links, previous)?;
    let missing = publish::check_links(prefix, &expected)?;
    if missing
        .iter()
        .any(|name| previous || journal.change.added_links.get(name) != expected.get(name))
    {
        return Err("Pending installation is missing owned command links".into());
    }
    for name in missing {
        std::os::unix::fs::symlink(&expected[&name], prefix.join(name))?;
    }
    publish::sync(&prefix.join("bin"))?;
    publish::sync(&prefix.join("share"))?;
    Ok(())
}
