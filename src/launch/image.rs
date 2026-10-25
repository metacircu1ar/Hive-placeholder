//! Build, cache, and reclaim private trusted launcher images.
use crate::{
    error::{Error, Result},
    os, state,
};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

mod retirement;
use retirement::{Boundary, Retirement};

#[cfg(test)]
use std::os::unix::fs::PermissionsExt;

pub(super) struct Image {
    pub(super) directory: PathBuf,
    pub(super) path: PathBuf,
    pub(super) digest: String,
    retirement: Retirement,
    owner: Option<state::Lock>,
    /// Named by its content's hash, so later processes running the same
    /// executable can share it instead of copying and assessing a new file.
    reusable: bool,
    /// Held with a shared lock: other processes may run it too, so this one
    /// never removes it.
    shared: bool,
    retained: bool,
}
impl Image {
    fn cleanup(&self) -> Result<()> {
        if self.shared {
            return Ok(());
        }
        let root = self.retirement.root()?;
        let _cache = state::Lock::acquire_existing(&root.join("cache.lock"))?;
        if let Some(owner) = &self.owner {
            self.retirement.remove(Some(owner))
        } else {
            self.retirement.remove_empty_reservation()
        }
    }
    /// After its self-check, let other processes share a reusable image.
    pub(super) fn share(&mut self) -> Result<()> {
        let Some(owner) = self
            .owner
            .as_ref()
            .filter(|_| self.reusable && !self.shared)
        else {
            return Ok(());
        };
        // Every other request on the owner lock happens under cache.lock, so
        // none can take it while flock converts the hold.
        let _cache = state::Lock::index(&self.retirement.root()?.join("cache.lock"))?;
        owner.share()?;
        self.shared = true;
        Ok(())
    }
    /// Give up an image that failed its self-check, removing it once no other
    /// process holds it.
    pub(super) fn reject(mut self) {
        if !self.shared {
            // The exclusive owner removes it on drop, as after a failed build.
            return;
        }
        self.retained = true;
        let directory = self.directory.clone();
        let root = self.retirement.root().map(Path::to_path_buf);
        drop(self);
        if let Ok(root) = root {
            discard(&root, &directory);
        }
    }
    fn failed(&mut self, primary: &Error) {
        use std::error::Error as _;
        self.retained = true; // Never silently retry a failed cleanup from Drop.
        let ambiguous = primary
            .source()
            .and_then(|error| error.downcast_ref::<atomic_fs::Error>())
            .is_some_and(|error| error.retained().is_some());
        if !ambiguous && let Err(secondary) = self.cleanup() {
            tracing::error!(name: "launch.cleanup_failed",
                 code = primary.code(),
                 primary = primary as &(dyn std::error::Error + 'static),
                 secondary = &secondary as &(dyn std::error::Error + 'static),
                 decision = "retain_image");
        }
    }
}
impl Drop for Image {
    fn drop(&mut self) {
        if !self.retained
            && !std::thread::panicking()
            && let Err(error) = self.cleanup()
        {
            tracing::warn!(name: "launch.drop_cleanup_failed",
                 code = error.code(),
                 error = &error as &(dyn std::error::Error + 'static),
                 decision = "retain_image");
        }
    }
}
pub(super) fn build_image(
    root: &Path,
    current: &Path,
    before_copy: impl FnOnce(&Path),
) -> Result<Image> {
    // Hash before taking cache.lock: builders never wait behind file I/O.
    let wanted = crate::executable::sha256(current)?;
    let name = format!("hive-launch-{wanted}");
    let cache = state::Lock::index(&root.join("cache.lock"))?;
    sweep_images(root, Some(&name));
    let content = root.join(&name);
    if retirement::exists(&content)? && !retirement::exists(&content.join("owner.lock"))? {
        // A builder died before owning it: every builder creates owner.lock
        // under cache.lock, so nobody else can still be filling it.
        if let Err(error) = Retirement::capture(&content)
            .and_then(|retirement| retirement.remove_empty_reservation())
        {
            warning(&error);
        }
    }
    if !retirement::exists(&content)? && retirement::matching_asides(&content)?.is_empty() {
        return match build_new(cache, root, &name, current, Some(&wanted), before_copy) {
            // The executable was replaced during the copy: build an unnamed image.
            Err(Error::UnsafeState { .. }) if !retirement::exists(&content)? => {
                let cache = state::Lock::index(&root.join("cache.lock"))?;
                build_new(cache, root, &unique_name(), current, None, |_| {})
            }
            built => built,
        };
    }
    // Under cache.lock no remover can run between this capture and the hold.
    let held = Retirement::capture(&content).and_then(|retirement| {
        state::Lock::shared_existing(&content.join("owner.lock")).map(|owner| (retirement, owner))
    });
    drop(cache);
    if let Ok((retirement, owner)) = held {
        match verify_reusable(&content, &wanted) {
            Ok(()) => {
                return Ok(Image {
                    path: content.join("hive-launch"),
                    directory: content,
                    digest: wanted,
                    retirement,
                    owner: Some(owner),
                    reusable: true,
                    shared: true,
                    retained: false,
                });
            }
            Err(error) => {
                warning(&error);
                drop(owner);
                discard(root, &content);
            }
        }
    }
    // Being built by another process, damaged, or blocked by an aside. When a
    // damaged image was just removed, build under the content name again.
    let cache = state::Lock::index(&root.join("cache.lock"))?;
    if !retirement::exists(&content)? && retirement::matching_asides(&content)?.is_empty() {
        return match build_new(cache, root, &name, current, Some(&wanted), before_copy) {
            // The executable was replaced during the copy: build an unnamed image.
            Err(Error::UnsafeState { .. }) if !retirement::exists(&content)? => {
                let cache = state::Lock::index(&root.join("cache.lock"))?;
                build_new(cache, root, &unique_name(), current, None, |_| {})
            }
            built => built,
        };
    }
    build_new(cache, root, &unique_name(), current, None, before_copy)
}
fn unique_name() -> String {
    format!("hive-launch-{}", uuid::Uuid::new_v4())
}
/// A cached image is reused only in its exact published shape.
fn verify_reusable(directory: &Path, wanted: &str) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    state::check_path(directory, true)?;
    let mut names = Vec::new();
    for entry in fs::read_dir(directory).map_err(Error::state_io_from)? {
        names.push(entry.map_err(Error::state_io_from)?.file_name());
    }
    names.sort();
    if names != ["hive-launch", "owner.lock"] {
        return Err(changed());
    }
    let path = directory.join("hive-launch");
    let metadata = state::private_reader(&path)?
        .metadata()
        .map_err(Error::state_io_from)?;
    if metadata.mode() & 0o7777 != os::READ_EXECUTE_MODE
        || crate::executable::sha256(&path)? != wanted
    {
        return Err(changed());
    }
    Ok(())
}
fn changed() -> Error {
    Error::unsafe_state("A cached launcher image no longer matches its content.")
}
/// Remove an image nobody holds; a held or unprovable one is left in place.
fn discard(root: &Path, directory: &Path) {
    let Ok(_cache) = state::Lock::index(&root.join("cache.lock")) else {
        return;
    };
    if let Err(error) = Retirement::capture(directory).and_then(|r| r.remove(None))
        && !matches!(error, Error::ProfileBusy { .. })
    {
        warning(&error);
    }
}
/// Build a new image named `name`, holding cache.lock until its owner is durable.
fn build_new(
    cache: state::Lock,
    root: &Path,
    name: &str,
    current: &Path,
    wanted: Option<&str>,
    before_copy: impl FnOnce(&Path),
) -> Result<Image> {
    let retirement = Retirement::allocate(root, name)?;
    let directory = retirement.original.clone();
    let mut image = Image {
        digest: String::new(),
        path: directory.join("hive-launch"),
        directory,
        retirement,
        owner: None,
        reusable: wanted.is_some(),
        shared: false,
        retained: false,
    };
    let initialized = initialize_owner(&mut image);
    // Metadata is durable; independent builders never wait behind copy I/O.
    drop(cache);
    if let Err(error) = initialized {
        image.failed(&error);
        return Err(error);
    }
    before_copy(&image.directory);
    let copied = copy_private_image(current, &image.path).and_then(|()| {
        image.retirement.check(&image.directory)?;
        state::check_path(&image.path, false)
    });
    if let Err(error) = copied {
        image.failed(&error);
        return Err(error);
    }
    match crate::executable::sha256(&image.path) {
        Ok(digest) => image.digest = digest,
        Err(error) => {
            image.failed(&error);
            return Err(error);
        }
    }
    if wanted.is_some_and(|wanted| wanted != image.digest) {
        // The executable changed during the copy: never publish it under
        // another content's name.
        let error = changed();
        image.failed(&error);
        return Err(error);
    }
    tracing::info!(name: "launch.image_built", success = true);
    Ok(image)
}
fn initialize_owner(image: &mut Image) -> Result<()> {
    #[cfg(test)]
    state::sync_tests::step(
        &image.directory,
        hive_setup_support::checkpoints::Checkpoint::BeforeIntent,
    )
    .map_err(Error::state_io_from)?;
    image.owner = Some(state::Lock::acquire(&image.directory.join("owner.lock"))?);
    image.retirement.durable_owner(
        image
            .owner
            .as_ref()
            .ok_or_else(|| Error::internal_error("Missing image owner."))?,
    )
}
pub(super) fn copy_private_image(current: &Path, path: &Path) -> Result<()> {
    let mut input = fs::File::open(current).map_err(Error::state_io_from)?;
    atomic_fs::PreparedCreate::new(
        path,
        (hive_setup_support::names::PUBLICATION_OPTIONS).with_mode(os::READ_EXECUTE_MODE),
    )
    .and_then(|prepared| {
        prepared.publish_stream(
            |output| {
                #[cfg(test)]
                copy_tests::boundary(path, output)?;
                io::copy(&mut input, output).map(|_| ())
            },
            &atomic_fs::DefaultBarriers,
        )
    })
    .map_err(crate::publication::error)
}
/// Remove images and asides nobody holds, except the image named `reusable`.
pub(super) fn sweep_images(root: &Path, reusable: Option<&str>) {
    if let Err(error) = sweep_checked(root, reusable) {
        warning(&error);
    }
}
fn warning(error: &Error) {
    tracing::warn!(name: "launch.sweep_failed",
         code = error.code(),
         error = error as &(dyn std::error::Error + 'static),
         decision = "retain_image");
}
fn sweep_checked(root: &Path, reusable: Option<&str>) -> Result<()> {
    for entry in fs::read_dir(root).map_err(Error::state_io_from)? {
        let entry = entry.map_err(Error::state_io_from)?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let result = if retirement::is_aside(name) {
            Retirement::parse(&entry.path()).and_then(|retirement| retirement.remove(None))
        } else if reusable == Some(name) {
            continue;
        } else if retirement::image_name(name) {
            sweep_image(&entry.path())
        } else {
            continue;
        };
        if let Err(error) = result
            && !matches!(error, Error::ProfileBusy { .. })
        {
            warning(&error);
        }
    }
    // Repeat even with no entries after a prior visible but unsynced final rmdir.
    Boundary(root).sync()
}
fn sweep_image(dir: &Path) -> Result<()> {
    if !retirement::exists(dir)? || !retirement::exists(&dir.join("owner.lock"))? {
        return Ok(());
    }
    Retirement::capture(dir)?.remove(None)
}

fn owned_entry(name: &str) -> bool {
    matches!(name, "owner.lock" | "hive-launch")
        || name.strip_prefix(".hive-tmp-").is_some_and(|suffix| {
            suffix.len() == 16
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

#[cfg(test)]
mod copy_tests;
