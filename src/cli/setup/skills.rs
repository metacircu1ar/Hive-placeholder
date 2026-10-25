//! Opt-in skill copies track exactly the bytes we own; user edits are preserved.
mod closure;
mod owned;
mod publication;
mod recovery;
mod removal;
#[cfg(test)]
mod tests;
use super::{Result, invalid};
use crate::config::Provider;
use hive_setup_support::files;
use publication::io;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
pub(super) struct Copies {
    groups: Vec<owned::Group>,
}
impl Copies {
    pub fn inspect(skills: &Path, providers: &[Provider]) -> Result<Self> {
        if providers.is_empty() {
            return Ok(Self { groups: vec![] });
        }
        let home = home()?;
        let product = skills
            .parent()
            .ok_or_else(|| invalid("Skills path needs a parent."))?;
        let bundle = closure::bundle(product)?;
        let mut groups = vec![];
        for provider in providers {
            for name in ["hive", "comb-curator"] {
                let target = base(&home, *provider).join(name);
                groups.push(owned::Group::inspect(target, desired(name, &bundle)?)?);
            }
        }
        Ok(Self { groups })
    }
    pub fn paths(&self) -> Vec<PathBuf> {
        self.groups.iter().flat_map(owned::Group::paths).collect()
    }
    pub fn apply(&self) -> Result<bool> {
        let mut changed = false;
        for group in &self.groups {
            changed |= group.apply()?;
        }
        Ok(changed)
    }
}
fn desired(name: &str, bundle: &BTreeMap<PathBuf, Vec<u8>>) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
    let key = PathBuf::from(format!("skills/{name}/SKILL.md"));
    let guide = bundle
        .get(&key)
        .ok_or_else(|| invalid("Distribution is missing a bundled skill."))?;
    let text = std::str::from_utf8(guide)
        .map_err(|cause| invalid("Skill is not UTF-8.").with_source(cause))?;
    let mut files = BTreeMap::from([(PathBuf::from("SKILL.md"), closure::relocate(&key, text)?)]);
    for (relative, bytes) in bundle {
        files.insert(Path::new("bundle").join(relative), bytes.clone());
    }
    Ok(files)
}
fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or_else(|| invalid("Skill copies require absolute HOME."))
}
fn base(home: &Path, provider: Provider) -> PathBuf {
    home.join(match provider {
        Provider::Codex => ".codex",
        Provider::Claude => ".claude",
    })
    .join("skills")
}
pub(in crate::cli) fn stale(skills: &Path) -> Result<Vec<PathBuf>> {
    let home = home()?;
    let mut targets = vec![];
    for provider in [Provider::Codex, Provider::Claude] {
        for name in ["hive", "comb-curator"] {
            let target = base(&home, provider).join(name);
            if owned::present(&target)? {
                targets.push((target, name));
            }
        }
    }
    if targets.is_empty() {
        return Ok(vec![]);
    }
    let bundle = closure::bundle(
        skills
            .parent()
            .ok_or_else(|| invalid("Skills path needs a parent."))?,
    )?;
    let mut stale = vec![];
    for (target, name) in targets {
        if owned::stale(&target, &desired(name, &bundle)?)? {
            stale.push(target);
        }
    }
    Ok(stale)
}
fn safe(path: &Path) -> Result<()> {
    if files::absolute(path).map_err(io)? != std::path::absolute(path).map_err(io)? {
        return Err(invalid(
            "User skill directory contains a symlink; refusing copied files.",
        ));
    }
    Ok(())
}
const LIMIT: u64 = 2 * 1024 * 1024;

fn read_owned(path: &Path) -> Result<Option<Vec<u8>>> {
    Ok(read_file(path)?.map(|(_, bytes)| bytes))
}

fn read_file(path: &Path) -> Result<Option<(std::fs::File, Vec<u8>)>> {
    use std::{
        fs::OpenOptions,
        io::Read,
        os::unix::fs::{MetadataExt, OpenOptionsExt},
    };
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io(e)),
    };
    let m = file.metadata().map_err(io)?;
    if !m.is_file()
        || m.nlink() != 1
        || m.uid() != rustix::process::getuid().as_raw()
        || m.mode() & 0o022 != 0
        || m.len() > LIMIT
    {
        return Err(invalid("Unsafe user skill file."));
    }
    let mut bytes = Vec::new();
    (&file)
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() as u64 > LIMIT {
        return Err(invalid("User skill file exceeds size limit."));
    }
    Ok(Some((file, bytes)))
}
