//! Bounded safe reads, private path checks and page-path access.
use super::{Disk, Durability};
use crate::{Error, Result, page, values::Slug};
use std::{
    fs::{self, File},
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::{Path, PathBuf},
};

pub(crate) fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
pub(crate) fn dir(path: &Path, create: bool, durability: &dyn Durability) -> Result<()> {
    if create && !path.exists() {
        fs::DirBuilder::new().mode(0o700).create(path)?;
        if let Some(parent) = path.parent() {
            durability.sync_directory(&File::open(parent)?)?;
        }
    }
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir()
        || meta.file_type().is_symlink()
        || meta.uid() != rustix::process::geteuid().as_raw()
        || !hive_setup_support::directories::owned(
            meta.uid(),
            meta.mode(),
            rustix::process::geteuid().as_raw(),
        )
    {
        return Err(Error::unsafe_directory());
    }
    check_name(&File::open(path)?, path)?;
    Ok(())
}
impl Disk {
    pub fn page_path(&self, slug: &Slug, create: bool) -> Result<PathBuf> {
        let mut path = self.root.clone();
        let device = self.root_device;
        let pieces: Vec<_> = slug.as_str().split('/').collect();
        for part in &pieces[..pieces.len() - 1] {
            path.push(part);
            match fs::symlink_metadata(&path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && !create => {}
                _ => {
                    dir(&path, create, self.durability.as_ref())?;
                    same_device(&path, device)?;
                }
            }
        }
        path.push(format!("{}.md", pieces[pieces.len() - 1]));
        if exists(&path)? {
            same_device(&path, device)?;
        }
        Ok(path)
    }
    pub fn content(&self, slug: &Slug) -> Result<Option<String>> {
        let path = self.page_path(slug, false)?;
        match read(&path, page::PAGE_BYTES) {
            Ok(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|cause| Error::page_not_utf8().caused_by(cause.utf8_error())),
            Err(error) if error.is_missing_file() => Ok(None),
            Err(error) => Err(error),
        }
    }
}

pub(super) fn same_device(path: &Path, expected: u64) -> Result<()> {
    if fs::symlink_metadata(path)?.dev() != expected {
        return Err(Error::cross_device_comb());
    }
    Ok(())
}

pub(super) fn create_root(path: &Path, durability: &dyn Durability) -> Result<()> {
    if exists(path)? {
        return Ok(());
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        create_root(parent, durability)?;
    }
    fs::DirBuilder::new().mode(0o700).create(path)?;
    durability.sync_directory(&File::open(path)?)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    durability.sync_directory(&File::open(parent)?)?;
    Ok(())
}

pub(crate) use comb_protocol::fs::{check_file, check_name, read};
