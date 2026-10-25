use super::*;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn explicit_cache_is_lazy_reuses_its_image_and_owns_fixture_resources() -> Result<()> {
    let root = crate::launch::tests::private_root().map_err(Error::state_io_from)?;
    let root_path = root.path().to_owned();
    let executable = root.path().join("source");
    fs::write(&executable, b"launcher fixture").map_err(Error::state_io_from)?;
    let phase = Arc::new(AtomicUsize::new(0));
    let root_phase = Arc::clone(&phase);
    let executable_phase = Arc::clone(&phase);
    let cache = LauncherCache::new(
        move || {
            assert_eq!(root_phase.fetch_add(1, Ordering::SeqCst), 0);
            Ok(root.path().to_owned())
        },
        move || {
            assert_eq!(executable_phase.fetch_add(1, Ordering::SeqCst), 1);
            Ok(executable.clone())
        },
    );
    assert_eq!(phase.load(Ordering::SeqCst), 0);
    let image = cache.image()?;
    assert_eq!(
        fs::read(&image).map_err(Error::state_io_from)?,
        b"launcher fixture"
    );
    assert_eq!(cache.image()?, image);
    assert_eq!(phase.load(Ordering::SeqCst), 2);
    drop(cache);
    assert!(!root_path.exists());
    Ok(())
}

#[test]
fn root_failure_precedes_binary_resolution() {
    let cache = LauncherCache::new(
        || Err(Error::invalid_path("fixture root unavailable")),
        || panic!("binary resolution ran before the root was available"),
    );
    assert!(matches!(cache.image(), Err(Error::InvalidPath { .. })));
}

#[test]
fn helpers_and_digest_keep_the_frozen_image_after_source_replacement() {
    let root = private_root_for_test();
    let source = root.path().join("source");
    fs::write(&source, b"original helper").unwrap();
    let path = source.clone();
    let directory = root.path().to_owned();
    let cache = LauncherCache::new(move || Ok(directory.clone()), move || Ok(path.clone()));
    let image = cache.image().unwrap();
    let digest = cache.digest().unwrap();
    fs::write(&source, b"new helper after update").unwrap();
    assert_eq!(cache.executable().unwrap(), image);
    assert_eq!(
        fs::read(cache.executable().unwrap()).unwrap(),
        b"original helper"
    );
    assert_eq!(cache.digest().unwrap(), digest);
    assert_ne!(crate::executable::sha256(&source).unwrap(), digest);
}
fn private_root_for_test() -> tempfile::TempDir {
    crate::launch::tests::private_root().unwrap()
}
