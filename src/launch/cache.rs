//! Lazy launcher dependencies and the lifetime of each private cached image.
use super::image::{Image, build_image};
use crate::{
    error::{Error, Result},
    os, state,
};
use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex, OnceLock},
};

type PathResolver = Box<dyn Fn() -> Result<PathBuf> + Send + Sync>;

pub(crate) struct LauncherCache {
    // Drop the image before resolver-owned resources such as a fixture's root.
    image: Mutex<Option<Image>>,
    root: PathResolver,
    executable: PathResolver,
}

impl LauncherCache {
    pub(crate) fn new(
        root: impl Fn() -> Result<PathBuf> + Send + Sync + 'static,
        executable: impl Fn() -> Result<PathBuf> + Send + Sync + 'static,
    ) -> Self {
        Self {
            image: Mutex::new(None),
            root: Box::new(root),
            executable: Box::new(executable),
        }
    }
    pub(crate) fn protected_root(&self) -> Result<PathBuf> {
        (self.root)()
    }
    pub(crate) fn executable(&self) -> Result<PathBuf> {
        // Hooks and shims must use the same immutable image as their daemon.
        self.image()
    }
    pub(crate) fn digest(&self) -> Result<String> {
        self.image()?;
        let image = self.image.lock().map_err(|_| state::recovery_required())?;
        image
            .as_ref()
            .map(|image| image.digest.clone())
            .ok_or_else(|| Error::internal_error("Missing frozen launcher image."))
    }
    pub(crate) fn check_workspace(&self, cwd: &Path) -> Result<()> {
        let root = self.protected_root()?;
        let cwd = cwd.canonicalize().map_err(Error::state_io_from)?;
        if root.starts_with(&cwd) || cwd.starts_with(root) {
            return Err(Error::invalid_cwd(
                "Agent workspace contains private launcher images.",
            ));
        }
        Ok(())
    }
    pub(crate) fn initialize(&self) -> Result<()> {
        os::reset_sigchld().map_err(Error::state_io_from)?;
        let ran = os::spawn(Command::new(self.image()?).arg("--hive-launch-self-check"))
            .and_then(|mut child| child.wait());
        let mut saved = self.image.lock().map_err(|_| state::recovery_required())?;
        if !ran.as_ref().is_ok_and(std::process::ExitStatus::success) {
            // Never keep, or let another process reuse, an image that cannot run.
            if let Some(image) = saved.take() {
                image.reject();
            }
            return Err(match ran {
                Err(error) => Error::state_io_from(error),
                Ok(_) => Error::spawn_failed("Trusted launcher self-check failed."),
            });
        }
        // A checked image may now be shared with later processes.
        if let Some(image) = saved.as_mut() {
            image.share()?;
        }
        Ok(())
    }
    pub(crate) fn image(&self) -> Result<PathBuf> {
        // Drop PoisonError's guard now; the error must not hold the image cache locked.
        let mut saved = self.image.lock().map_err(|_| state::recovery_required())?;
        if let Some(image) = saved.as_ref() {
            return Ok(image.path.clone());
        }
        let root = self.protected_root()?;
        let current = (self.executable)()?;
        let image = build_image(&root, &current, |_| {})?;
        let path = image.path.clone();
        *saved = Some(image);
        Ok(path)
    }
}

// Static images survive until OS exit releases their owner lock. The next
// command sweeps them; atexit cannot prove other threads finished spawning.
static SYSTEM: OnceLock<Arc<LauncherCache>> = OnceLock::new();
pub(crate) fn system_cache() -> Arc<LauncherCache> {
    Arc::clone(SYSTEM.get_or_init(|| {
        Arc::new(LauncherCache::new(system_root, || {
            crate::executable::path().map(Path::to_path_buf)
        }))
    }))
}

fn system_root() -> Result<PathBuf> {
    let home =
        PathBuf::from(env::var_os("HOME").ok_or_else(|| {
            Error::invalid_path("HOME is required for private launcher storage.")
        })?);
    if !home.is_absolute() {
        return Err(Error::invalid_path(
            "HOME must be absolute for private launcher storage.",
        ));
    }
    let home = home.canonicalize().map_err(Error::state_io_from)?;
    // A dedicated private child avoids relying on permissions of .local/state.
    let root = home.join(".hive-launch-images");
    state::directory(&root)?;
    for temp in [PathBuf::from("/tmp"), env::temp_dir()] {
        if root.starts_with(temp.canonicalize().map_err(Error::state_io_from)?) {
            return Err(Error::unsafe_state(
                "Launcher image HOME must be outside writable temporary directories.",
            ));
        }
    }
    Ok(root)
}

#[cfg(test)]
mod tests;
