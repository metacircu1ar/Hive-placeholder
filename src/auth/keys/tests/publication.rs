use super::*;

#[test]
fn unchanged_key_resolution_keeps_durable_proofs_fresh_during_publication() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let path = root.join("key");
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .write(&path, b"unchanged")
        .unwrap();
    let state = State::new(root.join("state")).unwrap();
    let binding = state
        .register(
            "api",
            &crate::config::Profile {
                provider: crate::config::Provider::Codex,
                method: crate::config::Method::ApiKey,
                api_key_env: None,
                api_key_file: Some(path),
            },
        )
        .unwrap();
    let keys = Keys::default();
    let first = keys.resolve(&state, &binding).unwrap();
    for fail in [false, true] {
        let proof = first.clone();
        let target = state.root.join("index.json");
        let observed = Arc::new(AtomicU64::new(0));
        let count = observed.clone();
        let result = crate::state::sync_tests::with_hook(
            move |path, step| {
                if path == target
                    && matches!(
                        step,
                        atomic_fs::Step::BeforeFileSync | atomic_fs::Step::BeforeDirectorySync
                    )
                {
                    // Inspect from another thread while resolution is inside
                    // the actual publisher, not only before/after it returns.
                    assert!(std::thread::scope(|scope| {
                        scope.spawn(|| proof.fresh()).join().unwrap()
                    }));
                    count.fetch_add(1, Ordering::Relaxed);
                    if fail && step == atomic_fs::Step::BeforeDirectorySync {
                        return Err(std::io::Error::from_raw_os_error(libc::EIO));
                    }
                }
                Ok(())
            },
            || keys.resolve(&state, &binding),
        );
        assert_eq!(observed.load(Ordering::Relaxed), 2);
        assert!(first.fresh());
        if fail {
            assert_eq!(result.err().unwrap().io_raw_os_error(), Some(libc::EIO));
        } else {
            let second = result.unwrap();
            assert!(second.fresh());
            assert_eq!(second.authority(), first.authority());
        }
    }
}

#[test]
fn published_but_unsynced_selection_revokes_proofs_until_retry_is_durable() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let path = root.join("key");
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .write(&path, b"first")
        .unwrap();
    let state = State::new(root.join("state")).unwrap();
    let binding = state
        .register(
            "api",
            &crate::config::Profile {
                provider: crate::config::Provider::Codex,
                method: crate::config::Method::ApiKey,
                api_key_env: None,
                api_key_file: Some(path.clone()),
            },
        )
        .unwrap();
    let keys = Keys::default();
    let first = keys.resolve(&state, &binding).unwrap();
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .write(&path, b"second")
        .unwrap();
    for at in [
        atomic_fs::Step::BeforeDirectorySync,
        atomic_fs::Step::BeforeFileSync,
        atomic_fs::Step::BeforeDirectorySync,
    ] {
        let target = state.root.join("index.json");
        let error = crate::state::sync_tests::with_hook(
            move |path, step| {
                if path == target && step == at {
                    Err(std::io::Error::from_raw_os_error(libc::EIO))
                } else {
                    Ok(())
                }
            },
            || keys.resolve(&state, &binding),
        )
        .err()
        .unwrap();
        assert_eq!(error.io_raw_os_error(), Some(libc::EIO));
        assert!(!first.fresh());
        let visible = state.key_authority(&binding).unwrap().unwrap();
        assert_eq!(visible.revision.get(), 2);
        assert_eq!(
            visible.fingerprint,
            crate::config::KeyFingerprint::digest(b"second")
        );
    }
    let second = keys.resolve(&state, &binding).unwrap();
    assert!(second.fresh());
    assert!(!first.fresh());
    assert_eq!(second.authority().unwrap().revision.get(), 2);
    // A new in-memory owner must also settle visible metadata before granting it.
    let restarted = Keys::default();
    let target = state.root.join("index.json");
    let error = crate::state::sync_tests::with_hook(
        move |path, step| {
            if path == target && step == atomic_fs::Step::BeforeDirectorySync {
                Err(std::io::Error::from_raw_os_error(libc::EIO))
            } else {
                Ok(())
            }
        },
        || restarted.resolve(&state, &binding),
    )
    .err()
    .unwrap();
    assert_eq!(error.io_raw_os_error(), Some(libc::EIO));
    assert!(restarted.resolve(&state, &binding).unwrap().fresh());
}
