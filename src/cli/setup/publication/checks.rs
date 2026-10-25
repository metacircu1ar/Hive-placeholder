//! Repeat durability barriers without changing an identical user configuration.
use super::{Barriers, Path, Publication, Snapshot, Step, config};
use std::{
    fs::{self, File},
    io,
    os::unix::fs::MetadataExt,
};

pub(super) fn resync(snapshot: &Snapshot) -> io::Result<()> {
    let (file, text) = config::read_file_text(&snapshot.target).map_err(io::Error::other)?;
    if text != snapshot.text {
        return Err(io::Error::other(super::invalid(
            "Configuration changed before synchronization.",
        )));
    }
    let parent = snapshot
        .target
        .parent()
        .ok_or_else(|| io::Error::other("Config needs a parent."))?;
    let directory = File::open(parent)?;
    let barriers = Publication {
        path: &snapshot.target,
        snapshot: None,
    };
    let check = || {
        snapshot.check().map_err(io::Error::other)?;
        same_name(&file, &snapshot.target)?;
        same_name(&directory, parent)
    };
    check()?;
    barriers.observe(Step::BeforeFileSync)?;
    check()?;
    barriers.sync_file(&file)?;
    barriers.observe(Step::AfterFileSync)?;
    check()?;
    barriers.observe(Step::BeforeDirectorySync)?;
    check()?;
    barriers.sync_directory(&directory)?;
    barriers.observe(Step::AfterDirectorySync)?;
    check()
}

fn same_name(file: &File, path: &Path) -> io::Result<()> {
    let held = file.metadata()?;
    let named = fs::symlink_metadata(path)?;
    if named.file_type().is_symlink()
        || (held.dev(), held.ino()) != (named.dev(), named.ino())
        || held.uid() != rustix::process::getuid().as_raw()
        || if held.is_dir() {
            !hive_setup_support::directories::owned(
                held.uid(),
                held.mode(),
                rustix::process::geteuid().as_raw(),
            )
        } else {
            held.mode() & 0o022 != 0
        }
        || (held.is_file() && held.nlink() != 1)
    {
        return Err(io::Error::other(super::invalid(
            "Configuration publication identity changed.",
        )));
    }
    Ok(())
}
