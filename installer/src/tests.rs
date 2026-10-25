//! Synthetic payloads exercise the installer transaction; V5 uses native binaries.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use manifest::{File, Manifest};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::Path,
};

pub(crate) fn fixture(root: &Path, part: &str) -> Manifest {
    let (component, names, suffix) = match part {
        "hive" => ("hive", vec!["hivectl", "hive-daemon"], "comb"),
        "hive-core" => ("hive", vec!["hivectl", "hive-daemon"], "core"),
        "comb" => ("comb", vec!["comb"], "http"),
        _ => unreachable!(),
    };
    let mut data: Vec<(String, Vec<u8>, u32)> = names
        .into_iter()
        .map(|name| {
            (
                format!("bin/{name}"),
                format!("#!/bin/sh\nprintf '%s\\n' 'fixture ({suffix})'\n").into_bytes(),
                0o755,
            )
        })
        .collect();
    data.push((
        format!("share/{component}/THIRD_PARTY_LICENSES"),
        b"fixture notices".to_vec(),
        0o644,
    ));
    data.push((
        format!("share/{component}/build.json"),
        b"{}".to_vec(),
        0o644,
    ));
    let mut files = BTreeMap::new();
    for (name, data, mode) in data {
        let path = root.join(part).join(&name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &data).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        files.insert(
            name,
            File {
                sha256: manifest::hash(&path).unwrap(),
                bytes: data.len() as u64,
                mode,
            },
        );
    }
    Manifest {
        format: hive_input::compatibility::Exact,
        product_version: env!("CARGO_PKG_VERSION").into(),
        part: part.into(),
        target: manifest::native_target().unwrap(),
        required_glibc: cfg!(target_os = "linux").then(|| "2.35".into()),
        files,
    }
}
fn install(prefix: &Path, staging: &Path, parts: &[Manifest]) {
    publish::install(prefix, staging, parts, |_| Ok(())).unwrap();
}
fn archive(source: &Path, staging: &Path, part: &Manifest, extra: Option<(&str, tar::EntryType)>) {
    let file = source.join(format!("{}-{}.tar.gz", part.part, part.target));
    let writer = flate2::write::GzEncoder::new(
        fs::File::create(&file).unwrap(),
        flate2::Compression::fast(),
    );
    let mut tar = tar::Builder::new(writer);
    for (name, entry) in &part.files {
        let mut header = tar::Header::new_ustar();
        header.set_size(entry.bytes);
        header.set_mode(entry.mode);
        header.set_cksum();
        tar.append_data(
            &mut header,
            name,
            fs::File::open(staging.join(&part.part).join(name)).unwrap(),
        )
        .unwrap();
    }
    let json = serde_json::to_vec(part).unwrap();
    let mut header = tar::Header::new_ustar();
    header.set_size(json.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    tar.append_data(&mut header, "manifest.json", json.as_slice())
        .unwrap();
    if let Some((name, kind)) = extra {
        let mut header = tar::Header::new_ustar();
        header.set_size(0);
        header.set_mode(0o644);
        header.set_entry_type(kind);
        if kind.is_symlink() {
            header.set_link_name("/tmp/unowned").unwrap();
        }
        header.set_cksum();
        tar.append_data(&mut header, name, &[][..]).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap();
    fs::write(
        source.join("SHA256SUMS"),
        format!(
            "{}  {}\n",
            manifest::hash(&file).unwrap(),
            file.file_name().unwrap().to_str().unwrap()
        ),
    )
    .unwrap();
}

#[test]
fn replacing_pair_preserves_standalone_comb_and_user_state() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefix");
    let capable = fixture(root.path(), "hive");
    let comb = fixture(root.path(), "comb");
    install(&prefix, root.path(), &[capable, comb]);
    let old = fs::read_link(prefix.join("lib/hive/current")).unwrap();
    let retained = prefix.join("bin/comb").canonicalize().unwrap();
    let inode = fs::metadata(&retained).unwrap().ino();
    fs::write(root.path().join("user-state"), b"login stays").unwrap();
    let core = fixture(root.path(), "hive-core");
    install(&prefix, root.path(), &[core]);
    assert_ne!(fs::read_link(prefix.join("lib/hive/current")).unwrap(), old);
    assert!(prefix.join("lib/hive").join(old).is_dir());
    assert_eq!(fs::metadata(prefix.join("bin/comb")).unwrap().ino(), inode);
    assert_eq!(
        fs::read(root.path().join("user-state")).unwrap(),
        b"login stays"
    );
    assert_eq!(
        prefix.join("bin/hivectl").canonicalize().unwrap().parent(),
        prefix
            .join("bin/hive-daemon")
            .canonicalize()
            .unwrap()
            .parent()
    );
}
#[test]
fn malformed_input_never_changes_previous_install() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefix");
    let capable = fixture(root.path(), "hive");
    install(&prefix, root.path(), &[capable]);
    let old = fs::read_link(prefix.join("lib/hive/current")).unwrap();
    let core = fixture(root.path(), "hive-core");
    fs::write(root.path().join("hive-core/bin/hivectl"), b"bad").unwrap();
    assert!(publish::install(&prefix, root.path(), &[core], |_| Ok(())).is_err());
    assert_eq!(fs::read_link(prefix.join("lib/hive/current")).unwrap(), old);
}
#[test]
fn unowned_commands_and_modified_payloads_are_refused() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefix");
    fs::create_dir_all(prefix.join("bin")).unwrap();
    fs::write(prefix.join("bin/hivectl"), "unrelated").unwrap();
    let part = fixture(root.path(), "hive");
    assert!(
        publish::install(
            &prefix,
            root.path(),
            std::slice::from_ref(&part),
            |_| Ok(())
        )
        .is_err()
    );
    assert_eq!(
        fs::read_to_string(prefix.join("bin/hivectl")).unwrap(),
        "unrelated"
    );
    fs::remove_file(prefix.join("bin/hivectl")).unwrap();
    install(&prefix, root.path(), std::slice::from_ref(&part));
    fs::write(prefix.join("bin/hivectl"), "edited").unwrap();
    assert!(publish::install(&prefix, root.path(), &[part], |_| Ok(())).is_err());
}
#[test]
fn symlinked_prefix_or_payload_ancestor_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("outside");
    fs::create_dir(&destination).unwrap();
    let prefix = root.path().join("prefix");
    symlink(&destination, &prefix).unwrap();
    let part = fixture(root.path(), "hive");
    assert!(publish::install(&prefix, root.path(), &[part], |_| Ok(())).is_err());
    assert_eq!(fs::read_dir(destination).unwrap().count(), 0);
}
#[test]
fn transaction_faults_restore_old_pointer_and_remove_only_new_links() {
    for point in [
        "before_replace",
        "after_replace",
        "before_sync",
        "after_sync",
        "link_created",
        "after_links",
    ] {
        let root = tempfile::tempdir().unwrap();
        let prefix = root.path().join("prefix");
        let first = fixture(root.path(), "hive");
        install(&prefix, root.path(), &[first]);
        let old = fs::read_link(prefix.join("lib/hive/current")).unwrap();
        let core = fixture(root.path(), "hive-core");
        let comb = fixture(root.path(), "comb");
        assert!(
            publish::install(&prefix, root.path(), &[core, comb], |at| if at == point {
                Err("injected failure".into())
            } else {
                Ok(())
            })
            .is_err()
        );
        assert_eq!(
            fs::read_link(prefix.join("lib/hive/current")).unwrap(),
            old,
            "{point}"
        );
        assert!(fs::symlink_metadata(prefix.join("bin/comb")).is_err());
        assert!(prefix.join("bin/hivectl").exists());
    }
}
#[test]
fn uncertain_rollback_keeps_both_generations_and_links() {
    for point in ["rollback", "rollback_sync"] {
        let root = tempfile::tempdir().unwrap();
        let prefix = root.path().join("prefix");
        let first = fixture(root.path(), "hive");
        install(&prefix, root.path(), &[first]);
        let core = fixture(root.path(), "hive-core");
        let comb = fixture(root.path(), "comb");
        let error = publish::install(&prefix, root.path(), &[core, comb], |at| {
            if at == "after_links" || at == point {
                Err("injected failure".into())
            } else {
                Ok(())
            }
        })
        .err()
        .unwrap();
        assert!(error.to_string().contains("Rollback uncertain"));
        let log = fs::read_to_string(prefix.join("lib/hive-install-logs/install.jsonl")).unwrap();
        assert!(log.contains("install.cleanup_failed"));
        assert!(!log.contains("injected failure"));
        assert!(!log.contains(root.path().to_str().unwrap()));
        assert_eq!(
            fs::read_dir(prefix.join("lib/hive/generations"))
                .unwrap()
                .count(),
            2
        );
        assert!(
            fs::symlink_metadata(prefix.join("bin/comb"))
                .unwrap()
                .is_symlink()
        );
    }
}

#[test]
fn completed_rollback_is_logged_without_native_cause_text() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefix");
    let part = fixture(root.path(), "hive");
    let error = publish::install(&prefix, root.path(), &[part], |at| {
        if at == "after_replace" {
            Err(std::io::Error::from_raw_os_error(28).into())
        } else {
            Ok(())
        }
    })
    .err()
    .unwrap();
    assert!(error.downcast_ref::<std::io::Error>().is_some());
    assert!(
        publish::current(&prefix.join("lib/hive"))
            .unwrap()
            .is_none()
    );
    let log = fs::read_to_string(prefix.join("lib/hive-install-logs/install.jsonl")).unwrap();
    assert!(log.contains("install.rollback_finished"));
    assert!(log.contains("install.cleanup_finished"));
    assert!(log.contains("install.failed"));
    assert!(!log.contains(root.path().to_str().unwrap()));
}
#[test]
fn first_install_rollback_can_be_retried() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefix");
    let part = fixture(root.path(), "hive");
    assert!(
        publish::install(&prefix, root.path(), std::slice::from_ref(&part), |at| {
            if at == "after_replace" {
                Err("injected".into())
            } else {
                Ok(())
            }
        })
        .is_err()
    );
    assert!(fs::symlink_metadata(prefix.join("lib/hive/current")).is_err());
    install(&prefix, root.path(), &[part]);
}
#[test]
fn verified_archive_unpacks_and_checksum_failures_do_not() {
    let root = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let staging = tempfile::tempdir().unwrap();
    let part = fixture(root.path(), "hive");
    archive(source.path(), root.path(), &part, None);
    archive::unpack(source.path(), staging.path(), "hive", &part.target).unwrap();
    fs::write(source.path().join("SHA256SUMS"), "invalid").unwrap();
    assert!(archive::unpack(source.path(), root.path(), "hive", &part.target).is_err());
}
#[test]
fn archive_rejects_symlinks_duplicates_and_unmanifested_files() {
    for entry in [
        ("bin/link", tar::EntryType::Symlink),
        ("bin/hivectl", tar::EntryType::Regular),
        ("unowned", tar::EntryType::Regular),
    ] {
        let root = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        let part = fixture(root.path(), "hive");
        archive(source.path(), root.path(), &part, Some(entry));
        assert!(archive::unpack(source.path(), staging.path(), "hive", &part.target).is_err());
    }
}
#[test]
fn unsafe_paths_and_unknown_formats_are_rejected() {
    for path in [
        "../escape",
        "/absolute",
        "bin//hive",
        "bin/./hive",
        "bin/../../escape",
        "bin\\escape",
    ] {
        assert!(!manifest::safe_path(path));
    }
    let root = tempfile::tempdir().unwrap();
    let part = fixture(root.path(), "hive");
    let mut json = serde_json::to_value(part).unwrap();
    json["format_version"] = 1.into();
    assert!(serde_json::from_value::<Manifest>(json).is_err());
}

#[test]
fn matching_but_unowned_link_is_not_adopted() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefix");
    fs::create_dir_all(prefix.join("bin")).unwrap();
    symlink(
        "../lib/hive/current/bin/hivectl",
        prefix.join("bin/hivectl"),
    )
    .unwrap();
    let part = fixture(root.path(), "hive");
    assert!(publish::install(&prefix, root.path(), &[part], |_| Ok(())).is_err());
    assert!(
        fs::symlink_metadata(prefix.join("bin/hivectl"))
            .unwrap()
            .is_symlink()
    );
}

#[test]
fn created_prefix_ancestors_are_synced_before_publication() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("first/second/prefix");
    let part = fixture(root.path(), "hive");
    let mut events = Vec::new();
    publish::install(&prefix, root.path(), &[part], |event| {
        events.push(event.to_owned());
        Ok(())
    })
    .unwrap();
    let publication = events
        .iter()
        .position(|event| event == "before_replace")
        .unwrap();
    for parent in [
        root.path().to_owned(),
        root.path().join("first"),
        root.path().join("first/second"),
        prefix.clone(),
        prefix.join("lib"),
        prefix.join("lib/hive"),
    ] {
        let label = format!("parent_synced:{}", parent.display());
        assert!(events[..publication].contains(&label), "{label}");
    }
}

#[test]
fn directory_sync_failure_cannot_publish_a_partial_installation() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("first/prefix");
    let part = fixture(root.path(), "hive");
    assert!(
        publish::install(&prefix, root.path(), &[part], |event| {
            if event.starts_with("parent_synced:") {
                Err("injected directory sync failure".into())
            } else {
                Ok(())
            }
        })
        .is_err()
    );
    assert!(fs::symlink_metadata(prefix.join("lib/hive/current")).is_err());
}

#[test]
fn loader_failures_report_requirements_status_and_stderr_before_mutation() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefix");
    let mut part = fixture(root.path(), "comb");
    let binary = root.path().join("comb/bin/comb");
    fs::write(
        &binary,
        "#!/bin/sh\nprintf '%s\n' 'GLIBC_2.39 not found in libc.so.6' >&2\nexit 1\n",
    )
    .unwrap();
    let file = part.files.get_mut("bin/comb").unwrap();
    file.bytes = fs::metadata(&binary).unwrap().len();
    file.sha256 = manifest::hash(&binary).unwrap();
    let error = publish::install(&prefix, root.path(), &[part], |_| Ok(()))
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("stderr: GLIBC_2.39 not found"));
    assert!(error.contains("exit status: 1"));
    assert!(!prefix.exists());
}

#[test]
fn non_executable_staging_names_tmpdir_remedy() {
    let root = tempfile::tempdir().unwrap();
    let part = fixture(root.path(), "hive");
    fs::set_permissions(
        root.path().join("hive/bin/hivectl"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let error = part
        .check_versions(&root.path().join("hive"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("TMPDIR") && error.contains("noexec"));
}

#[test]
fn retry_syncs_existing_managed_directory_entries_before_publication() {
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefix");
    let part = fixture(root.path(), "hive");
    assert!(
        publish::install(&prefix, root.path(), std::slice::from_ref(&part), |event| {
            if event == "before_replace" {
                Err("interrupted first install".into())
            } else {
                Ok(())
            }
        })
        .is_err()
    );
    assert!(prefix.join("lib/hive").is_dir());
    let mut events = Vec::new();
    publish::install(&prefix, root.path(), &[part], |event| {
        events.push(event.to_owned());
        Ok(())
    })
    .unwrap();
    let publication = events
        .iter()
        .position(|event| event == "before_replace")
        .unwrap();
    for parent in [&prefix, &prefix.join("lib")] {
        assert!(events[..publication].contains(&format!("parent_synced:{}", parent.display())));
    }
}

#[test]
fn traversable_unreadable_ancestor_does_not_block_installation() {
    let root = tempfile::tempdir().unwrap();
    let ancestor = root.path().join("traversable");
    fs::create_dir_all(ancestor.join("owned")).unwrap();
    fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o311)).unwrap();
    let prefix = ancestor.join("owned/prefix");
    let part = fixture(root.path(), "hive");
    let mut events = Vec::new();
    let installed = publish::install(&prefix, root.path(), &[part], |event| {
        events.push(event.to_owned());
        Ok(())
    });
    // Restore before assertions so a failure still allows fixture cleanup.
    fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o700)).unwrap();
    installed.unwrap();
    let publication = events
        .iter()
        .position(|event| event == "before_replace")
        .unwrap();
    for parent in [ancestor.join("owned"), prefix.clone(), prefix.join("lib")] {
        assert!(events[..publication].contains(&format!("parent_synced:{}", parent.display())));
    }
}

#[test]
fn guide_covers_native_installer_and_bootstrap_flags() {
    use clap::CommandFactory;
    let guide = include_str!("../../docs/setup.md");
    for arg in crate::options::Args::command()
        .get_arguments()
        .filter(|a| !a.is_hide_set())
    {
        if let Some(long) = arg.get_long() {
            assert!(
                guide
                    .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                    .any(|word| word == format!("--{long}")),
                "Undocumented installer --{long}"
            );
        }
    }
    let script = include_str!("../../packaging/install.sh");
    for line in script
        .lines()
        .take_while(|line| !line.starts_with("# A function"))
    {
        let Some(arm) = line.trim().strip_suffix(')') else {
            continue;
        };
        for flag in arm.split('|').filter(|s| {
            s.starts_with("--") && s.chars().all(|c| c == '-' || c.is_ascii_alphanumeric())
        }) {
            assert!(guide.contains(flag), "Undocumented bootstrap {flag}");
        }
    }
}

#[test]
fn invalid_prefix_precedes_any_staged_payload_validation() {
    let directory = tempfile::tempdir().unwrap();
    let mut manifest = fixture(directory.path(), "hive");
    manifest.target = "not-the-native-target".into();
    let error = publish::install(Path::new("relative"), directory.path(), &[manifest], |_| {
        Ok(())
    })
    .err()
    .unwrap();
    let input = error.downcast_ref::<hive_input::InputError>().unwrap();
    assert_eq!(input.field(), "--prefix");
    assert!(!directory.path().join("relative").exists());
}
