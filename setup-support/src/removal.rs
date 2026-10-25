//! Exact object receipts for retryable deletion under product-owned locks.
use crate::{Result, files, invalid};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    device: u64,
    inode: u64,
    kind: u32,
}
impl Identity {
    pub fn is_regular_file(&self) -> bool {
        self.kind == 0o100_000
    }
    pub fn read(path: &Path) -> Result<Option<Self>> {
        let meta = match fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if meta.file_type().is_symlink() || meta.uid() != rustix::process::geteuid().as_raw() {
            return Err(invalid(format!(
                "Refusing unowned or symlinked purge object: {}",
                path.display()
            )));
        }
        Ok(Some(Self {
            device: meta.dev(),
            inode: meta.ino(),
            kind: meta.mode() & 0o170_000,
        }))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Object {
    pub path: PathBuf,
    #[serde(deserialize_with = "Option::deserialize")]
    pub identity: Option<Identity>,
}
impl Object {
    pub fn capture(path: &Path) -> Result<Self> {
        let path = safe_path(path)?;
        Ok(Self {
            identity: Identity::read(&path)?,
            path,
        })
    }
    pub fn check(&self) -> Result<()> {
        if safe_path(&self.path)? != self.path || Identity::read(&self.path)? != self.identity {
            return Err(invalid(format!(
                "Purge object changed since admission; refusing {}",
                self.path.display()
            )));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    pub object: Object,
    pub quarantine: PathBuf,
    pub done: bool,
}
impl Action {
    pub fn capture(path: &Path, nonce: &str) -> Result<Self> {
        if nonce.is_empty()
            || !nonce
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(invalid("Invalid purge identity."));
        }
        let object = Object::capture(path)?;
        let name = object
            .path
            .file_name()
            .ok_or_else(|| invalid("Purge object needs a name."))?
            .to_string_lossy();
        let quarantine = object
            .path
            .with_file_name(format!(".{name}.hive-purge-{nonce}"));
        if files::optional_metadata(&quarantine)?.is_some() {
            return Err(invalid("Purge quarantine already exists."));
        }
        Ok(Self {
            object,
            quarantine,
            done: false,
        })
    }
    /// The admitted object identity is also the quarantine identity after rename.
    /// Persist the complete receipt before calling this, and after marking done.
    pub fn remove(&mut self) -> Result<()> {
        self.validate()?;
        let current = Identity::read(&self.object.path)?;
        let quarantined = Identity::read(&self.quarantine)?;
        if self.done {
            if current.is_some() || quarantined.is_some() {
                return Err(invalid(
                    "A completed purge path was recreated; refusing a second deletion.",
                ));
            }
            return Ok(());
        }
        if self.object.identity.is_none() {
            self.object.check()?;
            if quarantined.is_some() {
                return Err(invalid("Unadmitted quarantine object."));
            }
        } else if quarantined.is_some() {
            if quarantined != self.object.identity || current.is_some() {
                return Err(invalid("Purge quarantine or original path was replaced."));
            }
            remove_object(&self.quarantine)?;
        } else if current.is_some() {
            self.object.check()?;
            fs::rename(&self.object.path, &self.quarantine)?;
            files::sync_parent(&self.quarantine)?;
            if Identity::read(&self.quarantine)? != self.object.identity {
                return Err(invalid("Purge quarantine identity changed."));
            }
            remove_object(&self.quarantine)?;
        }
        if self.object.path.parent().is_some_and(Path::exists) {
            files::sync_parent(&self.object.path)?;
        }
        self.done = true;
        Ok(())
    }
    pub fn validate(&self) -> Result<()> {
        if safe_path(&self.object.path)? != self.object.path
            || self.quarantine.parent() != self.object.path.parent()
            || !self.quarantine.file_name().is_some_and(|n| {
                n.to_string_lossy().starts_with(&format!(
                    ".{}.hive-purge-",
                    self.object
                        .path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                ))
            })
        {
            return Err(invalid("Invalid purge action scope."));
        }
        Ok(())
    }
}
fn remove_object(path: &Path) -> Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    files::sync_parent(path)
}
pub fn safe_path(path: &Path) -> Result<PathBuf> {
    let path = files::absolute(path)?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|p| files::location(&p))
        .transpose()?;
    if path.parent().is_none()
        || path.components().count() < 3
        || home
            .as_ref()
            .is_some_and(|h| h == &path || h.starts_with(&path))
        || [
            "/usr",
            "/usr/local",
            "/opt",
            "/tmp",
            "/var",
            "/Users",
            "/home",
        ]
        .iter()
        .any(|p| path == Path::new(p))
    {
        return Err(invalid(format!(
            "Refusing broad purge path: {}",
            path.display()
        )));
    }
    Ok(path)
}
pub fn disjoint(paths: &[&Path]) -> Result<()> {
    for (i, path) in paths.iter().enumerate() {
        for other in &paths[i + 1..] {
            if path.starts_with(other) || other.starts_with(path) {
                return Err(invalid(
                    "Purge paths overlap; select independent product locations.",
                ));
            }
        }
    }
    Ok(())
}
/// Validate an owned product subtree without following links or crossing devices.
pub fn tree(path: &Path) -> Result<()> {
    let Some(identity) = Identity::read(path)? else {
        return Ok(());
    };
    let mut pending = vec![path.to_owned()];
    while let Some(path) = pending.pop() {
        let next = Identity::read(&path)?
            .ok_or_else(|| invalid("Purge tree changed during inspection."))?;
        if next.device != identity.device {
            return Err(invalid("Purge cannot cross a mount point."));
        }
        let meta = fs::symlink_metadata(&path)?;
        if meta.is_dir() {
            for entry in fs::read_dir(path)? {
                pending.push(entry?.path());
            }
        } else if !meta.is_file() && next.kind != 0o140_000 {
            return Err(invalid("Purge tree contains an unsupported object."));
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn retry_removes_only_admitted_quarantine_and_refuses_recreation() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("owned");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("data"), b"data").unwrap();
        let mut action = Action::capture(&path, "test").unwrap();
        fs::rename(&path, &action.quarantine).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(action.remove().is_err());
        fs::remove_dir(&path).unwrap();
        action.remove().unwrap();
        action.remove().unwrap();
        fs::create_dir(&path).unwrap();
        assert!(action.remove().is_err());
    }
    #[test]
    fn absent_admission_never_deletes_a_later_object() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("later");
        let mut action = Action::capture(&path, "absent").unwrap();
        fs::write(&path, b"keep").unwrap();
        assert!(action.remove().is_err());
        assert_eq!(fs::read(path).unwrap(), b"keep");
    }
}
