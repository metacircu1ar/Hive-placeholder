//! One durable pointer publishes the complete command set; old images stay valid.
use crate::{
    Result,
    manifest::{self, Installed, Manifest},
    publication,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
};

pub(crate) const OWNER: &[u8] = b"Hive managed installation\n";
pub struct Report {
    pub generations: usize,
    pub bytes: u64,
}

fn directory(path: &Path, hook: &mut impl FnMut(&str) -> Result<()>) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(_) => Err(format!(
            "Refusing non-directory installation path: {}",
            path.display()
        )
        .into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path)?;
            sync_parent(path, hook)?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}
fn sync_parent(path: &Path, hook: &mut impl FnMut(&str) -> Result<()>) -> Result<()> {
    let parent = path
        .parent()
        .ok_or("Installation directory has no parent")?;
    sync(parent)?;
    hook(&format!("parent_synced:{}", parent.display()))
}
fn prefix_directories(prefix: &Path, hook: &mut impl FnMut(&str) -> Result<()>) -> Result<()> {
    if !prefix.is_absolute() || prefix.is_symlink() {
        return Err("Installation prefix must be absolute and not a symlink".into());
    }
    let existing = prefix
        .ancestors()
        .find(|path| path.is_dir())
        .ok_or("Installation prefix has no existing ancestor")?;
    fs::create_dir_all(prefix)?;
    // Also cover ancestors from an interrupted first attempt. Read-only mounts
    // cannot contain directory entries newly created by this installer.
    for parent in prefix.ancestors().skip(1) {
        match fs::File::open(parent).and_then(|file| file.sync_all()) {
            Ok(()) => hook(&format!("parent_synced:{}", parent.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::ReadOnlyFilesystem => {}
            // Traversable ancestors may deny listing/opening. This invocation
            // created no entry above the nearest previously existing directory.
            Err(error)
                if error.kind() == std::io::ErrorKind::PermissionDenied
                    && parent != existing
                    && existing.starts_with(parent) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
pub(crate) fn sync(path: &Path) -> Result<()> {
    fs::File::open(path)?.sync_all()?;
    Ok(())
}
pub(crate) fn manager(prefix: &Path, hook: &mut impl FnMut(&str) -> Result<()>) -> Result<PathBuf> {
    if !prefix.is_absolute() || prefix.is_symlink() {
        return Err("Installation prefix must be absolute and not a symlink".into());
    }
    prefix_directories(prefix, hook)?;
    directory(&prefix.join("lib"), hook)?;
    let manager = prefix.join("lib/hive");
    match fs::symlink_metadata(&manager) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            publication::manager(&manager, hook)?;
        }
        Ok(_) => {
            if !fs::symlink_metadata(&manager)?.is_dir()
                || manifest::read_bounded(&manager.join("owner"), 128)? != OWNER
            {
                return Err("Refusing unrelated or incomplete installation manager".into());
            }
            if !fs::symlink_metadata(manager.join("generations"))?.is_dir() {
                return Err("Invalid generations directory".into());
            }
        }
        Err(error) => return Err(error.into()),
    }
    // Publication can have succeeded before its parent barrier failed. Reuse
    // repeats the complete-manager barrier before any generation is selected.
    sync(&manager.join("owner"))?;
    sync(&manager.join("generations"))?;
    sync_parent(&manager.join("generations"), hook)?;
    sync_parent(&manager, hook)?;
    Ok(manager)
}
pub(crate) fn current(manager: &Path) -> Result<Option<PathBuf>> {
    match fs::read_link(manager.join("current")) {
        Ok(path) => {
            let parts: Vec<_> = path.components().collect();
            if parts.len() != 2
                || !manifest::safe_path(path.to_str().ok_or("Invalid current path")?)
                || parts[0].as_os_str() != "generations"
                || !parts[1].as_os_str().to_string_lossy().starts_with("g-")
            {
                return Err("Invalid owned current pointer".into());
            }
            if !fs::symlink_metadata(manager.join(&path))?.is_dir() {
                return Err("Current generation is not a real directory".into());
            }
            Ok(Some(path))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
pub(crate) fn load(manager: &Path, pointer: Option<&Path>, target: &str) -> Result<Installed> {
    let Some(pointer) = pointer else {
        return Ok(Installed {
            format: hive_input::compatibility::Exact,
            target: target.into(),
            retired: BTreeMap::new(),
            parts: BTreeMap::new(),
            installer_notices: manifest::File {
                sha256: String::new(),
                bytes: 0,
                mode: 0o644,
            },
            installer: manifest::File {
                sha256: String::new(),
                bytes: 0,
                mode: 0o755,
            },
        });
    };
    let root = manager.join(pointer);
    let installed: Installed = serde_json::from_slice(&manifest::read_bounded(
        &root.join("installation.json"),
        4 * 1024 * 1024,
    )?)?;
    if installed.target != target || installed.parts.len() > 2 || installed.retired.len() > 2 {
        return Err("Unsupported installation manifest".into());
    }
    for (component, part) in &installed.parts {
        part.validate(target)?;
        if component != part.component()? {
            return Err("Invalid component ownership".into());
        }
        part.check_files(&root)?;
    }
    check_installer(&root, &installed.installer)?;
    check_notices(&root, &installed.installer_notices)?;
    for (component, pointer) in &installed.retired {
        retired_part(manager, pointer, component, target)?;
    }
    Ok(installed)
}
pub(crate) fn links(parts: &BTreeMap<String, Manifest>) -> Result<BTreeMap<PathBuf, PathBuf>> {
    let mut links = BTreeMap::new();
    if !parts.is_empty() {
        links.insert(
            "bin/hive-install".into(),
            "../lib/hive/current/bin/hive-install".into(),
        );
    }
    for (component, part) in parts {
        for binary in part.binaries()? {
            links.insert(
                PathBuf::from(format!("bin/{binary}")),
                PathBuf::from(format!("../lib/hive/current/bin/{binary}")),
            );
        }
        links.insert(
            PathBuf::from(format!("share/{component}")),
            PathBuf::from(format!("../lib/hive/current/share/{component}")),
        );
    }
    Ok(links)
}
pub(crate) fn check_links(
    prefix: &Path,
    expected: &BTreeMap<PathBuf, PathBuf>,
) -> Result<Vec<PathBuf>> {
    let mut missing = Vec::new();
    for (name, target) in expected {
        let path = prefix.join(name);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_symlink() && fs::read_link(&path)? == *target => {}
            Ok(_) => {
                return Err(format!(
                    "Refusing to overwrite unowned command or docs: {}",
                    path.display()
                )
                .into());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(name.clone());
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(missing)
}
fn populate(
    root: &Path,
    source: &Path,
    parts: &BTreeMap<String, Manifest>,
    selected: &[Manifest],
    old: Option<&Path>,
) -> Result<()> {
    // Validate both sets before copying any payload: two malformed part names
    // must never become equal by discarding their component errors.
    let selected_components = selected
        .iter()
        .map(Manifest::component)
        .collect::<Result<std::collections::BTreeSet<_>>>()?;
    let components = parts
        .values()
        .map(|part| Ok((part, part.component()?)))
        .collect::<Result<Vec<_>>>()?;
    for (part, component) in components {
        let replacing = selected_components.contains(component);
        let origin = if replacing {
            source.join(&part.part)
        } else {
            old.ok_or("Missing retained generation")?.to_owned()
        };
        for (name, file) in &part.files {
            let destination = root.join(name);
            fs::create_dir_all(destination.parent().ok_or("Missing payload parent")?)?;
            if replacing {
                publication::copy(&origin.join(name), &destination, file.mode)?;
            } else {
                // Never chmod, truncate or modify a retained shared inode.
                fs::hard_link(origin.join(name), &destination)?;
            }
            sync(&destination)?;
        }
        part.check_files(root)?;
        part.check_versions(root)?;
    }
    Ok(())
}
fn sync_tree(root: &Path) -> Result<()> {
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if fs::symlink_metadata(&path)?.is_dir() {
            sync_tree(&path)?;
        }
    }
    sync(root)
}

pub(crate) fn replace(manager: &Path, target: &Path) -> Result<()> {
    replace_with(manager, target, &atomic_fs::DefaultBarriers)
}
pub(crate) fn replace_with(
    manager: &Path,
    target: &Path,
    barriers: &impl atomic_fs::Barriers,
) -> Result<()> {
    let directory = tempfile::Builder::new()
        .prefix(".pointer-")
        .tempdir_in(manager)?;
    let parent = fs::File::open(directory.path())?;
    let link = directory.path().join("next");
    let mut published = false;
    let result = (|| {
        symlink(target, &link)?;
        barriers.sync_directory(&parent)?;
        fs::rename(link, manager.join("current"))?;
        published = true;
        // Both source and destination directory barriers are required. The
        // transaction syncs manager before exposing added links or restoring service.
        barriers.sync_directory(&parent)
    })();
    result.map_err(|source| atomic_fs::Error::new(source, published).into())
}
pub(crate) fn rollback(
    manager: &Path,
    old: Option<&Path>,
    hook: &mut impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    hook("rollback")?;
    if let Some(target) = old {
        replace(manager, target)?;
    } else if hive_setup_support::files::optional_metadata(&manager.join("current"))?.is_some() {
        fs::remove_file(manager.join("current"))?;
    }
    hook("rollback_sync")?;
    sync(manager)?;
    if current(manager)?.as_deref() != old {
        return Err("Rollback pointer could not be confirmed".into());
    }
    tracing::info!(name: "install.rollback_finished", confirmed = true);
    Ok(())
}
fn remove_links(
    prefix: &Path,
    names: &[PathBuf],
    expected: &BTreeMap<PathBuf, PathBuf>,
) -> Result<()> {
    for name in names {
        let path = prefix.join(name);
        if fs::read_link(&path)? != expected[name] {
            return Err("New command link changed during rollback".into());
        }
        fs::remove_file(path)?;
    }
    sync(&prefix.join("bin"))?;
    sync(&prefix.join("share"))?;
    Ok(())
}

type Links = BTreeMap<PathBuf, PathBuf>;
fn prepare_links(
    prefix: &Path,
    installed: &mut Installed,
    selected: &[Manifest],
    hook: &mut impl FnMut(&str) -> Result<()>,
) -> Result<(Links, Vec<PathBuf>)> {
    let previously_owned = links(&installed.parts)?;
    directory(&prefix.join("bin"), hook)?;
    directory(&prefix.join("share"), hook)?;
    if !check_links(prefix, &previously_owned)?.is_empty() {
        return Err("Owned command links are missing; refusing partial installation".into());
    }
    for part in selected {
        installed.retired.remove(part.component()?);
        installed
            .parts
            .insert(part.component()?.into(), part.clone());
    }
    let expected = links(&installed.parts)?;
    for name in expected.keys() {
        if !previously_owned.contains_key(name)
            && hive_setup_support::files::optional_metadata(&prefix.join(name))?.is_some()
        {
            return Err(format!(
                "Unowned or interrupted-install link requires inspection: {}",
                prefix.join(name).display()
            )
            .into());
        }
    }
    let missing = check_links(prefix, &expected)?;
    Ok((expected, missing))
}

#[cfg(test)]
pub fn install(
    prefix: &Path,
    source: &Path,
    selected: &[Manifest],
    hook: impl FnMut(&str) -> Result<()>,
) -> Result<Option<Report>> {
    install_with(prefix, source, selected, &mut crate::lifecycle::Inert, hook)
}

pub fn install_with(
    prefix: &Path,
    source: &Path,
    selected: &[Manifest],
    lifecycle: &mut dyn crate::lifecycle::Lifecycle,
    mut hook: impl FnMut(&str) -> Result<()>,
) -> Result<Option<Report>> {
    let target = validate_sources(prefix, source, selected)?;
    let mut observed = |phase: &str| {
        crate::diagnostics::phase(phase);
        hook(phase)
    };
    prefix_directories(prefix, &mut observed)?;
    directory(&prefix.join("lib"), &mut observed)?;
    let owner = std::sync::Arc::new(installation_lock(&prefix.join("lib/hive"))?);
    crate::purge::fence_install(prefix)?;
    let manager = manager(prefix, &mut observed)?;
    crate::diagnostics::owned(prefix, owner, || {
        install_owned(
            prefix,
            source,
            selected,
            lifecycle,
            &manager,
            &target,
            &mut observed,
        )
    })
}

fn install_owned(
    prefix: &Path,
    source: &Path,
    selected: &[Manifest],
    lifecycle: &mut dyn crate::lifecycle::Lifecycle,
    manager: &Path,
    target: &str,
    mut hook: impl FnMut(&str) -> Result<()>,
) -> Result<Option<Report>> {
    lifecycle.recover(prefix, manager)?;
    let old = current(manager)?;
    let mut installed = load(manager, old.as_deref(), target)?;
    let (expected, missing) = prepare_links(prefix, &mut installed, selected, &mut hook)?;
    // A previous interrupted attempt may have created these entries without
    // syncing their parents. Repeat these syncs even for an existing layout.
    sync_parent(&prefix.join("lib"), &mut hook)?;
    sync_parent(manager, &mut hook)?;
    let generation = stage_generation(manager, source, &mut installed, selected, old.as_deref())?;
    tracing::info!(name: "install.generation_staged", success = true);
    let pointer = generation.strip_prefix(manager)?.to_owned();
    let preparation = lifecycle.prepare(&crate::lifecycle::Change {
        operation: crate::lifecycle::Operation::Install,
        removed_links: BTreeMap::new(),
        prefix: prefix.into(),
        manager: manager.into(),
        previous: old.clone(),
        next: pointer.clone(),
        added_links: missing
            .iter()
            .map(|name| (name.clone(), expected[name].clone()))
            .collect(),
        components: selected
            .iter()
            .map(|p| p.component().map(str::to_owned))
            .collect::<Result<_>>()?,
    });
    if let Err(error) = preparation {
        let cleanup = publication::discard_unreferenced(manager, &generation);
        return Err(publication::failed(
            error,
            "Preparation cleanup failed",
            cleanup,
        ));
    }
    let mut created = Vec::new();
    let result = (|| {
        lifecycle.stop()?;
        activate(
            prefix,
            manager,
            &pointer,
            &expected,
            &missing,
            &mut created,
            &mut hook,
        )?;
        for part in installed.parts.values() {
            part.check_versions(&prefix.join("lib/hive/current"))?;
        }
        tracing::info!(name: "install.versions_verified", success = true);
        Ok(())
    })();
    if let Err(error) = result {
        if let Err(uncertain) = rollback(manager, old.as_deref(), &mut hook) {
            return Err(publication::failed(
                error,
                "Rollback uncertain; generations and links retained",
                Err(uncertain),
            ));
        }
        let cleanup = (|| {
            remove_links(prefix, &created, &expected)?;
            // Clear the journal before deleting anything it may still reference.
            lifecycle.finish(false)?;
            publication::discard_unreferenced(manager, &generation)
        })();
        return Err(publication::failed(
            error,
            "Rollback cleanup or service restoration failed",
            cleanup,
        ));
    }
    lifecycle.finish(true).map_err(|error| {
        publication::context(
            "New binaries are installed, but service restart is incomplete. Repeat the \
            installation to recover",
            error,
        )
    })?;
    Ok(installed_usage(manager))
}

// A failed advisory count is unknown, never a fabricated zero or failed install.
fn installed_usage(manager: &Path) -> Option<Report> {
    match usage(&manager.join("generations")) {
        Ok(report) => Some(report),
        Err(error) => {
            tracing::warn!(name: "install.usage_unavailable",
                 error = error.as_ref() as &(dyn std::error::Error + 'static),
                 decision = "retain_successful_installation");
            None
        }
    }
}
fn validate_sources(prefix: &Path, source: &Path, selected: &[Manifest]) -> Result<String> {
    crate::options::input::validate_prefix(prefix)?;
    let target = manifest::native_target()?;
    // Even direct native-helper use validates all parts before prefix creation.
    for part in selected {
        part.validate(&target)?;
        part.check_files(&source.join(&part.part))?;
        part.check_versions(&source.join(&part.part))?;
    }
    Ok(target)
}
fn activate(
    prefix: &Path,
    manager: &Path,
    pointer: &Path,
    expected: &Links,
    missing: &[PathBuf],
    created: &mut Vec<PathBuf>,
    hook: &mut impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    hook("before_replace")?;
    replace(manager, pointer)?;
    hook("after_replace")?;
    hook("before_sync")?;
    sync(manager)?;
    hook("after_sync")?;
    for name in missing {
        symlink(&expected[name], prefix.join(name))?;
        created.push(name.clone());
        hook("link_created")?;
    }
    sync(&prefix.join("bin"))?;
    sync(&prefix.join("share"))?;
    hook("after_links")?;
    Ok(())
}
pub(crate) fn usage(root: &Path) -> Result<Report> {
    fn visit(path: &Path, seen: &mut BTreeSet<(u64, u64)>, bytes: &mut u64) -> Result<()> {
        for entry in fs::read_dir(path)? {
            let path = entry?.path();
            let meta = fs::symlink_metadata(&path)?;
            if meta.is_dir() {
                visit(&path, seen, bytes)?;
            } else if meta.is_file() && seen.insert((meta.dev(), meta.ino())) {
                *bytes += meta.len();
            }
        }
        Ok(())
    }
    let generations =
        fs::read_dir(root)?.try_fold(0_usize, |count, entry| entry.map(|_| count + 1))?;
    let mut bytes = 0;
    visit(root, &mut BTreeSet::new(), &mut bytes)?;
    Ok(Report { generations, bytes })
}

fn copy_installer(root: &Path) -> Result<manifest::File> {
    let source = std::env::current_exe()?;
    let destination = root.join("bin/hive-install");
    fs::create_dir_all(root.join("bin"))?;
    publication::copy(&source, &destination, 0o755)?;
    Ok(manifest::File {
        bytes: manifest::regular(&destination)?.len(),
        sha256: manifest::hash(&destination)?,
        mode: 0o755,
    })
}
fn check_installer(root: &Path, expected: &manifest::File) -> Result<()> {
    let path = root.join("bin/hive-install");
    if !fs::symlink_metadata(root.join("bin"))?.is_dir() {
        return Err("Installer parent is not a real directory".into());
    }
    let meta = manifest::regular(&path)?;
    if expected.mode != 0o755
        || meta.permissions().mode() & 0o7777 != expected.mode
        || meta.len() != expected.bytes
        || manifest::hash(&path)? != expected.sha256
    {
        return Err("Owned installation helper was modified".into());
    }
    Ok(())
}

pub(crate) fn stage_generation(
    manager: &Path,
    source: &Path,
    installed: &mut Installed,
    selected: &[Manifest],
    old: Option<&Path>,
) -> Result<PathBuf> {
    let generation = tempfile::Builder::new()
        .prefix("g-")
        .tempdir_in(manager.join("generations"))?;
    populate(
        generation.path(),
        source,
        &installed.parts,
        selected,
        old.map(|p| manager.join(p)).as_deref(),
    )?;
    installed.installer = copy_installer(generation.path())?;
    installed.installer_notices = copy_notices(generation.path())?;
    publication::write(
        &generation.path().join("installation.json"),
        &serde_json::to_vec_pretty(installed)?,
    )?;
    sync_tree(generation.path())?;
    // Keep before publication: no RAII cleanup may remove a possibly referenced image.
    let generation = generation.keep();
    sync(&manager.join("generations"))?;
    Ok(generation)
}

pub(crate) fn installation_lock(manager: &Path) -> Result<hive_setup_support::locks::Guard> {
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits().try_into()?)
        .open(
            manager
                .parent()
                .ok_or("Missing installation parent")?
                .join(".hive-install.lock"),
        )?;
    let metadata = lock.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::getuid().as_raw()
        || metadata.nlink() != 1
        || metadata.mode() & 0o022 != 0
    {
        return Err("Unsafe installation lock".into());
    }
    let guard = hive_setup_support::locks::Guard::exclusive(lock).map_err(|error| {
        publication::context("Cannot acquire installation ownership", Box::new(error))
    })?;
    Ok(guard)
}

fn copy_notices(root: &Path) -> Result<manifest::File> {
    #[cfg(not(test))]
    let bytes = {
        let mut source = std::env::current_exe()?.into_os_string();
        source.push(".LICENSES");
        manifest::read_bounded(Path::new(&source), 16 * 1024 * 1024).map_err(|e| {
            publication::context(
                "Installer license notices must accompany the executable as NAME.LICENSES",
                e,
            )
        })?
    };
    #[cfg(test)]
    let bytes = b"Installer test fixture notices".to_vec();
    let path = root.join("bin/hive-install.LICENSES");
    publication::write(&path, &bytes)?;
    Ok(manifest::File {
        bytes: bytes.len() as u64,
        sha256: manifest::hash(&path)?,
        mode: 0o644,
    })
}
fn check_notices(root: &Path, expected: &manifest::File) -> Result<()> {
    let path = root.join("bin/hive-install.LICENSES");
    let meta = manifest::regular(&path)?;
    if expected.mode != 0o644
        || meta.permissions().mode() & 0o7777 != expected.mode
        || meta.len() != expected.bytes
        || manifest::hash(&path)? != expected.sha256
    {
        return Err("Owned installer notices were modified".into());
    }
    Ok(())
}

pub(crate) fn retired_part(
    manager: &Path,
    pointer: &Path,
    component: &str,
    target: &str,
) -> Result<Manifest> {
    if pointer.components().count() != 2
        || !pointer.starts_with("generations")
        || pointer
            .file_name()
            .is_none_or(|p| !p.to_string_lossy().starts_with("g-"))
        || !pointer.to_str().is_some_and(manifest::safe_path)
    {
        return Err("Invalid retired generation pointer".into());
    }
    let root = manager.join(pointer);
    if !fs::symlink_metadata(&root)?.is_dir() {
        return Err("Retired image is not a directory".into());
    }
    let image: Installed = serde_json::from_slice(&manifest::read_bounded(
        &root.join("installation.json"),
        4 * 1024 * 1024,
    )?)?;
    if image.target != target {
        return Err("Retired image target mismatch".into());
    }
    let part = image
        .parts
        .get(component)
        .ok_or("Retired image lacks the removed component")?;
    part.validate(target)?;
    if part.component()? != component {
        return Err("Retired component ownership mismatch".into());
    }
    part.check_files(&root)?;
    Ok(part.clone())
}

#[cfg(test)]
mod component_tests {
    use super::*;

    #[test]
    fn invalid_selected_or_retained_component_never_copies_a_payload() {
        let root = tempfile::tempdir().unwrap();
        let valid = crate::tests::fixture(root.path(), "hive");
        let mut invalid = valid.clone();
        invalid.part = "unknown".into();
        // If comparison discards both errors, this tree would be copied before
        // check_files notices that the component name is invalid.
        fs::rename(root.path().join("hive"), root.path().join("unknown")).unwrap();
        for (parts, selected) in [
            (vec![invalid.clone()], vec![invalid.clone()]),
            (vec![valid.clone()], vec![invalid.clone()]),
            (vec![valid.clone(), invalid], vec![valid]),
        ] {
            let parts = parts
                .into_iter()
                .enumerate()
                .map(|(i, part)| (i.to_string(), part))
                .collect();
            let destination = root.path().join("destination");
            let result = populate(&destination, root.path(), &parts, &selected, None);
            assert_eq!(result.unwrap_err().to_string(), "Unknown release part");
            assert!(!destination.exists());
        }
    }
}
