//! Serialize cooperating setup writers and preserve user configuration symlinks.
mod checks;
#[cfg(test)]
pub(super) mod tests;
use super::{Result, config, invalid};
use atomic_fs::{Barriers, PreparedCreate, Step};
use hive_setup_support::files;
use std::path::{Path, PathBuf};

fn io(cause: std::io::Error) -> crate::error::Error {
    let message = "Cannot safely access the setup configuration output.";
    let public = if cause.kind() == std::io::ErrorKind::InvalidInput {
        invalid(message)
    } else {
        crate::error::Error::state_io(message)
    };
    public.with_source(cause)
}

pub(in crate::cli) struct Snapshot {
    original: PathBuf,
    target: PathBuf,
    pub text: String,
    existed: bool,
}
impl Snapshot {
    pub fn inspect(path: &Path) -> Result<Self> {
        let target = files::location(path).map_err(io)?;
        hive_setup_support::names::check_output_name(&target).map_err(io)?;
        let text = config::read_text(&target)?;
        Ok(Self {
            original: path.into(),
            target,
            existed: text.is_some(),
            text: text.unwrap_or_default(),
        })
    }
    pub fn target(&self) -> &Path {
        &self.target
    }
    pub fn publish(&self, text: &str) -> Result<Option<PathBuf>> {
        if text.len() as u64 > config::LIMIT {
            return Err(invalid("Setup configuration exceeds 1 MiB."));
        }
        crate::config::Config::parse_at(text, &self.target)?;
        let parent = self
            .target
            .parent()
            .ok_or_else(|| invalid("Config needs a parent directory."))?;
        files::directory(parent).map_err(io)?;
        let _lock =
            hive_setup_support::locks::path(&self.target, "hive-setup", true).map_err(io)?;
        self.check()?;
        if self.existed && text == self.text {
            checks::resync(self)
                .map_err(|source| error(atomic_fs::Error::new(source, true), false))?;
            return Ok(None);
        }
        let backup = self.backup(parent)?;
        let barriers = Publication {
            path: &self.target,
            snapshot: Some(self),
        };
        if self.existed {
            atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
                .with_barriers(&barriers)
                .write(&self.target, text.as_bytes())
        } else {
            PreparedCreate::new(&self.target, hive_setup_support::names::PUBLICATION_OPTIONS)
                .and_then(|prepared| prepared.publish_with(text.as_bytes(), &barriers))
        }
        .map_err(|cause| error(cause, false))?;
        Ok(backup)
    }
    fn backup(&self, parent: &Path) -> Result<Option<PathBuf>> {
        if !self.existed {
            return Ok(None);
        }
        let name = self
            .target
            .file_name()
            .ok_or_else(|| invalid("Config needs a filename."))?
            .to_string_lossy();
        let path = parent.join(format!(
            "{name}.backup-{}-{}",
            crate::state::now(),
            uuid::Uuid::new_v4()
        ));
        let barriers = Publication {
            path: &path,
            snapshot: Some(self),
        };
        PreparedCreate::new(&path, hive_setup_support::names::PUBLICATION_OPTIONS)
            .and_then(|prepared| prepared.publish_with(self.text.as_bytes(), &barriers))
            .map_err(|cause| error(cause, true))?;
        Ok(Some(path))
    }
    fn check(&self) -> Result<()> {
        if files::location(&self.original).map_err(io)? != self.target
            || config::read_text(&self.target)?.as_deref()
                != self.existed.then_some(self.text.as_str())
        {
            return Err(invalid(
                "Configuration or its symlink target changed during setup; rerun before \
                    publishing.",
            ));
        }
        Ok(())
    }
}

fn error(cause: atomic_fs::Error, backup: bool) -> crate::error::Error {
    if let Some(public) = cause
        .io_error()
        .get_ref()
        .and_then(|e| e.downcast_ref::<crate::error::Error>())
    {
        return public.clone().with_source(cause);
    }
    let message = if backup {
        "Configuration backup publication failed; the configuration was not \
            changed. Inspect the backup before retrying."
    } else if cause.published() {
        "Configuration is published, but its durability could not be confirmed. \
            Inspect the configuration before retrying setup."
    } else {
        "Configuration publication failed before replacement; retry setup after \
            correcting the cause."
    };
    let public = if (!backup && cause.published())
        || cause.io_error().kind() == std::io::ErrorKind::InvalidInput
        || cause.io_error().get_ref().is_some_and(
            <dyn std::error::Error + std::marker::Send + std::marker::Sync + 'static>::is::<
                atomic_fs::ConcurrentChange,
            >,
        ) {
        invalid(message)
    } else {
        crate::error::Error::state_io(message)
    };
    public.with_source(cause)
}

struct Publication<'a> {
    path: &'a Path,
    snapshot: Option<&'a Snapshot>,
}
impl Barriers for Publication<'_> {
    fn observe(&self, step: Step) -> std::io::Result<()> {
        #[cfg(not(test))]
        let _ = self.path;
        #[cfg(test)]
        tests::checkpoint(self.path, step)?;
        if step == Step::BeforePublish
            && let Some(snapshot) = self.snapshot
        {
            snapshot.check().map_err(std::io::Error::other)?;
        }
        Ok(())
    }
}
