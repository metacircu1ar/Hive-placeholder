use super::*;
use std::{fs, io, os::unix::fs::PermissionsExt};

fn private_directory(parent: &Path) -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(parent)
        .unwrap()
}

#[test]
fn typed_stream_error_and_ambiguous_cleanup_survive_outer_directory_drop() {
    let root = tempfile::tempdir().unwrap();
    let directory = private_directory(root.path());
    let retained = directory.path().to_owned();
    let original = Error::recovery_required("Fixture source changed.")
        .with_source(io::Error::from_raw_os_error(libc::EIO));
    let result: Result<(tempfile::TempDir, ())> = prepare(directory, |path| {
        atomic_fs::PreparedCreate::new(
            path.join("final"),
            hive_setup_support::names::PUBLICATION_OPTIONS,
        )
        .unwrap()
        .publish_stream(
            |output| {
                output.write_all(b"partial")?;
                let temporary = fs::read_dir(path)?.next().unwrap()?.path();
                fs::remove_file(&temporary)?;
                fs::write(&temporary, b"replacement")?;
                Err(io::Error::other(original.clone()))
            },
            &atomic_fs::DefaultBarriers,
        )
        .map_err(error)
    });
    let error = result.unwrap_err();
    assert_eq!(
        serde_json::to_value(&error).unwrap(),
        serde_json::to_value(&original).unwrap()
    );
    let failure = error
        .source()
        .unwrap()
        .downcast_ref::<atomic_fs::Error>()
        .unwrap();
    assert!(!failure.published() && failure.cleanup_error().is_some());
    let nested = failure
        .io_error()
        .get_ref()
        .unwrap()
        .downcast_ref::<Error>()
        .unwrap();
    assert_eq!(nested.io_raw_os_error(), Some(libc::EIO));
    assert_eq!(
        fs::read(failure.retained().as_ref().unwrap()).unwrap(),
        b"replacement"
    );
    assert!(retained.is_dir());
    assert!(!retained.join("final").exists());
}

#[test]
fn cleanly_failed_private_preparation_is_removed() {
    let root = tempfile::tempdir().unwrap();
    let directory = private_directory(root.path());
    let path = directory.path().to_owned();
    let result: Result<(tempfile::TempDir, ())> = prepare(directory, |path| {
        atomic_fs::PreparedCreate::new(
            path.join("final"),
            hive_setup_support::names::PUBLICATION_OPTIONS,
        )
        .unwrap()
        .publish_stream(
            |output| {
                output.write_all(b"partial")?;
                Err(io::Error::from_raw_os_error(libc::ENOSPC))
            },
            &atomic_fs::DefaultBarriers,
        )
        .map_err(error)
    });
    let failure = result.unwrap_err();
    let failure = failure
        .source()
        .unwrap()
        .downcast_ref::<atomic_fs::Error>()
        .unwrap();
    assert_eq!(failure.io_error().raw_os_error(), Some(libc::ENOSPC));
    assert!(!path.exists());
}

mod death;

#[test]
fn replacement_outer_directory_is_preserved_on_error_success_and_panic() {
    for outcome in ["error", "success", "panic"] {
        let root = tempfile::tempdir().unwrap();
        let directory = private_directory(root.path());
        let original = directory.path().to_owned();
        let moved = root.path().join("moved");
        let result = std::panic::catch_unwind(|| {
            prepare(directory, |path| {
                let reserved = atomic_fs::PreparedCreate::new(
                    path.join("final"),
                    hive_setup_support::names::PUBLICATION_OPTIONS,
                )
                .unwrap();
                fs::rename(path, &moved).unwrap();
                crate::state::directory(path).unwrap();
                fs::write(path.join("sentinel"), b"unrelated").unwrap();
                assert!(outcome != "panic", "owned constructor panic fixture");
                if outcome == "success" {
                    reserved.cancel().unwrap();
                    return Ok(());
                }
                reserved.publish(b"complete").map_err(error)
            })
        });
        if outcome == "panic" {
            assert!(result.is_err());
        } else {
            assert!(result.unwrap().is_err());
        }
        assert_eq!(fs::read(original.join("sentinel")).unwrap(), b"unrelated");
        assert_eq!(fs::read_dir(&moved).unwrap().count(), 0);
        assert!(!original.join("final").exists());
    }
}

#[test]
fn unwind_preserves_replaced_reservation_inside_unchanged_outer_directory() {
    let root = tempfile::tempdir().unwrap();
    let directory = private_directory(root.path());
    let path = directory.path().to_owned();
    let result = std::panic::catch_unwind(|| {
        prepare::<()>(directory, |path| {
            atomic_fs::PreparedCreate::new(
                path.join("final"),
                hive_setup_support::names::PUBLICATION_OPTIONS,
            )
            .unwrap()
            .publish_stream(
                |output| {
                    output.write_all(b"partial")?;
                    let temporary = fs::read_dir(path)?.next().unwrap()?.path();
                    fs::remove_file(&temporary)?;
                    fs::write(&temporary, b"replacement")?;
                    panic!("owned reservation unwind fixture");
                },
                &atomic_fs::DefaultBarriers,
            )
            .map_err(error)
        })
    });
    assert!(result.is_err());
    let entries: Vec<_> = fs::read_dir(&path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(fs::read(&entries[0]).unwrap(), b"replacement");
    assert!(!path.join("final").exists());
}
