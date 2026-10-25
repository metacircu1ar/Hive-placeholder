use crate::{Result, manifest, publish};
use hive_setup_support::{
    files,
    removal::{Action, Identity},
};
use std::{
    collections::BTreeSet,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

pub(super) fn remove(action: &mut Action) -> Result<()> {
    remove_with_scan(action, hive_setup_support::process_images::ensure_unused)
}
fn remove_with_scan(
    action: &mut Action,
    mut scan: impl FnMut(&[&Path]) -> std::io::Result<()>,
) -> Result<()> {
    action.validate()?;
    if action.done {
        action.remove()?;
        return Ok(());
    }
    let original = &action.object.path;
    let mut renamed = false;
    if files::optional_metadata(original)?.is_some() {
        action.object.check()?;
        validate(original)?;
        scan(&[original, &action.quarantine])?;
        if files::optional_metadata(&action.quarantine)?.is_some() {
            return Err("Image quarantine already exists".into());
        }
        fs::rename(original, &action.quarantine)?;
        renamed = true;
        files::sync_parent(original)?;
    } else if Identity::read(&action.quarantine)?.is_none() {
        // The caller durably recorded image admission before the first rename.
        action.remove()?;
        return Ok(());
    } else if Identity::read(&action.quarantine)? != action.object.identity {
        return Err("Image manager/quarantine identity changed; refusing deletion".into());
    }
    if let Err(error) = scan(&[original, &action.quarantine]) {
        let restore = (|| -> Result<()> {
            if renamed {
                if files::optional_metadata(original)?.is_some()
                    || Identity::read(&action.quarantine)? != action.object.identity
                {
                    return Err("Image restore identity changed; retain quarantine".into());
                }
                fs::rename(&action.quarantine, original)?;
                files::sync_parent(original)?;
            }
            Ok(())
        })();
        return Err(crate::publication::failed(
            error.into(),
            "Image restore failed",
            restore,
        ));
    }
    action.remove()?;
    Ok(())
}
pub(super) fn validate(manager: &Path) -> Result<()> {
    let device = fs::symlink_metadata(manager)?.dev();
    Identity::read(manager)?.ok_or("Installation manager disappeared")?;
    let generations = manager.join("generations");
    Identity::read(&generations)?.ok_or("Generations directory disappeared")?;
    if fs::symlink_metadata(&generations)?.dev() != device {
        return Err("Generations directory is on another filesystem".into());
    }
    if manifest::read_bounded(&manager.join("owner"), 128)? != publish::OWNER {
        return Err("Unrecognized installation owner".into());
    }
    let expected = BTreeSet::from(["owner".into(), "current".into(), "generations".into()]);
    let entries: BTreeSet<String> = fs::read_dir(manager)?
        .map(|e| {
            Ok(e?
                .file_name()
                .into_string()
                .map_err(|_| "Invalid manager filename")?)
        })
        .collect::<Result<_>>()?;
    if entries != expected {
        return Err("Unknown installation manager entries; image purge refused".into());
    }
    let current = publish::current(manager)?.ok_or("Missing current generation")?;
    let target = manifest::native_target()?;
    if !publish::load(manager, Some(&current), &target)?
        .parts
        .is_empty()
    {
        return Err("Active components remain; image purge refused".into());
    }
    for entry in fs::read_dir(manager.join("generations"))? {
        let path = entry?.path();
        if !path
            .file_name()
            .is_some_and(|s| s.to_string_lossy().starts_with("g-"))
            || !fs::symlink_metadata(&path)?.is_dir()
        {
            return Err("Unrecognized generation directory".into());
        }
        let installed = publish::load(manager, Some(path.strip_prefix(manager)?), &target)?;
        let mut expected = BTreeSet::from([
            PathBuf::from("installation.json"),
            PathBuf::from("bin/hive-install"),
            PathBuf::from("bin/hive-install.LICENSES"),
        ]);
        for part in installed.parts.values() {
            expected.extend(part.files.keys().map(PathBuf::from));
        }
        let directories: BTreeSet<_> = expected
            .iter()
            .flat_map(|p| p.ancestors().skip(1).map(Path::to_path_buf))
            .collect();
        let mut observed = BTreeSet::new();
        inventory(&path, &path, device, &directories, &mut observed)?;
        if expected != observed {
            return Err(format!("Unknown files in retained generation {}", path.display()).into());
        }
    }
    Ok(())
}
fn inventory(
    root: &Path,
    path: &Path,
    device: u64,
    directories: &BTreeSet<PathBuf>,
    observed: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    Identity::read(path)?.ok_or("Generation path disappeared")?;
    let metadata = fs::symlink_metadata(path)?;
    if metadata.dev() != device {
        return Err("Generation contains another filesystem".into());
    }
    if metadata.is_dir() {
        if !directories.contains(path.strip_prefix(root)?) {
            return Err("Unknown directory in retained generation".into());
        }
        for entry in fs::read_dir(path)? {
            inventory(root, &entry?.path(), device, directories, observed)?;
        }
    } else {
        observed.insert(path.strip_prefix(root)?.into());
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn full_image_purge_removes_all_generations_and_retains_external_lock() {
        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("prefix");
        let part = crate::tests::fixture(temp.path(), "hive");
        publish::install(&prefix, temp.path(), &[part], |_| Ok(())).unwrap();
        let diagnostic = prefix.join("lib/hive-install-logs/install.jsonl");
        assert!(
            fs::read_to_string(&diagnostic)
                .unwrap()
                .contains("install.finished")
        );
        crate::uninstall::remove_with(
            &prefix,
            &["hive".into()],
            true,
            false,
            &mut crate::lifecycle::Inert,
            |_| Ok(()),
        )
        .unwrap();
        let manager = prefix.join("lib/hive");
        let mut action = Action::capture(&manager, "images").unwrap();
        validate(&manager).unwrap();
        remove(&mut action).unwrap();
        assert!(!manager.exists());
        assert!(prefix.join("lib/.hive-install.lock").is_file());
        assert!(diagnostic.is_file());
        remove(&mut action).unwrap();
        assert!(!manager.exists());
        assert!(diagnostic.is_file());
    }
    #[test]
    fn unknown_generation_files_refuse_image_purge() {
        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("prefix");
        let part = crate::tests::fixture(temp.path(), "hive");
        publish::install(&prefix, temp.path(), &[part], |_| Ok(())).unwrap();
        crate::uninstall::remove_with(
            &prefix,
            &["hive".into()],
            true,
            false,
            &mut crate::lifecycle::Inert,
            |_| Ok(()),
        )
        .unwrap();
        let manager = prefix.join("lib/hive");
        let generation = manager.join(publish::current(&manager).unwrap().unwrap());
        fs::create_dir(generation.join("personal-empty-directory")).unwrap();
        assert!(validate(&manager).is_err());
        fs::remove_dir(generation.join("personal-empty-directory")).unwrap();
        fs::write(generation.join("personal"), b"keep").unwrap();
        let mut action = Action::capture(&manager, "images").unwrap();
        assert!(remove(&mut action).is_err());
        assert_eq!(fs::read(generation.join("personal")).unwrap(), b"keep");
    }
    #[test]
    fn failed_post_rename_scan_keeps_primary_when_restore_also_fails() {
        for obstruct in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let prefix = temp.path().join("prefix");
            let part = crate::tests::fixture(temp.path(), "hive");
            publish::install(&prefix, temp.path(), &[part], |_| Ok(())).unwrap();
            crate::uninstall::remove_with(
                &prefix,
                &["hive".into()],
                true,
                false,
                &mut crate::lifecycle::Inert,
                |_| Ok(()),
            )
            .unwrap();
            let manager = prefix.join("lib/hive");
            let mut action = Action::capture(&manager, "images").unwrap();
            let mut calls = 0;
            let error = remove_with_scan(&mut action, |_| {
                calls += 1;
                if calls == 1 {
                    return Ok(());
                }
                if obstruct {
                    fs::create_dir(&manager)?;
                    fs::write(manager.join("foreign"), b"keep")?;
                }
                Err(std::io::Error::from_raw_os_error(
                    rustix::io::Errno::NOSPC.raw_os_error(),
                ))
            })
            .unwrap_err();
            let mut cause: &(dyn std::error::Error + 'static) = error.as_ref();
            while let Some(source) = cause.source() {
                cause = source;
            }
            assert_eq!(
                cause
                    .downcast_ref::<std::io::Error>()
                    .unwrap()
                    .raw_os_error(),
                Some(rustix::io::Errno::NOSPC.raw_os_error())
            );
            assert_eq!(action.quarantine.exists(), obstruct);
            assert!(!action.done);
            if obstruct {
                assert_eq!(fs::read(manager.join("foreign")).unwrap(), b"keep");
            } else {
                validate(&manager).unwrap();
            }
        }
    }
    #[test]
    fn partial_quarantine_survives_scan_refusal_then_finishes() {
        let temp = tempfile::tempdir().unwrap();
        let manager = temp.path().join("manager");
        fs::create_dir(&manager).unwrap();
        fs::write(manager.join("remaining-image"), b"partial cleanup").unwrap();
        let mut action = Action::capture(&manager, "images").unwrap();
        fs::rename(&manager, &action.quarantine).unwrap();
        assert!(
            remove_with_scan(&mut action, |_| Err(std::io::Error::other("live process"))).is_err()
        );
        assert!(!manager.exists());
        assert!(action.quarantine.join("remaining-image").exists());
        remove_with_scan(&mut action, |_| Ok(())).unwrap();
        assert!(action.done);
        assert!(!action.quarantine.exists());
    }
}
