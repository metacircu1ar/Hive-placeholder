use super::*;

#[test]
fn batch_acceptance_and_completion_failures_keep_their_durable_boundary() {
    for occurrence in [1, 2] {
        for (position, step) in STEPS.into_iter().enumerate() {
            let mut fixture = Fixture::new();
            let error = fixture.fail("import.json", step, occurrence);
            let accepted = occurrence == 2 || position >= 4;
            assert_eq!(error.file_committed, accepted, "{occurrence}/{step:?}");
            assert_eq!(error.import_progress.is_some(), accepted);
            if let Some(progress) = error.import_progress {
                assert_eq!(progress.completed, if occurrence == 2 { 3 } else { 0 });
            }
            let mut fixture = fixture.reopen();
            let before = fixture.comb.import_status().unwrap().current;
            let result = fixture.apply().unwrap();
            if let Some(before) = before {
                assert_eq!(before.batch_id, result.batch_id);
            }
            assert_eq!(result.total, 3);
        }
    }
}

#[test]
fn unfinished_page_is_not_counted_until_ordinary_recovery_completes() {
    let mut fixture = Fixture::new();
    let recording = Arc::new(Recording::default());
    recording.fail_finish_once();
    fixture.comb.disk.set_durability(recording);
    let error = fixture.apply().unwrap_err();
    let progress = error.import_progress.unwrap();
    assert!(error.file_committed);
    assert_eq!(progress.completed, 0);
    assert_eq!(progress.unresolved.unwrap().as_str(), "a");
    assert_eq!(
        fixture.comb.import_status().unwrap_err().code,
        ErrorCode::RecoveryRequired
    );
    let mut fixture = fixture.reopen();
    assert_eq!(fixture.status().completed, 1);
    fixture.apply().unwrap();
    assert_eq!(fixture.status().completed, 3);
}

#[test]
fn rolled_back_page_retracts_tentative_receipt_before_batch_progress() {
    let mut fixture = Fixture::new();
    let recording = Arc::new(Recording::default());
    recording.fail_finish_once();
    fixture.comb.disk.set_durability(recording);
    fixture.apply().unwrap_err();
    // Simulate the page write not surviving while its receipt did.
    fs::remove_file(fixture.page("a")).unwrap();
    let mut fixture = fixture.reopen();
    assert_eq!(fixture.status().completed, 0);
    assert!(!fixture.page("a").exists());
    fixture.apply().unwrap();
    assert_eq!(fixture.status().completed, 3);
}

#[test]
fn every_native_import_barrier_failure_is_resumable_without_duplicate_pages() {
    let mut baseline = Fixture::new();
    let recording = Arc::new(Recording::default());
    recording.barrier(Ok(()));
    baseline.comb.disk.set_durability(recording.clone());
    baseline.apply().unwrap();
    let events = recording.counts().events;
    for operation in [
        Operation::Fsync,
        Operation::FullFlush,
        Operation::BeforeFinish,
    ] {
        for occurrence in 1..=events.iter().filter(|&&op| op == operation).count() {
            let mut fixture = Fixture::new();
            let recording = Arc::new(Recording::default());
            recording.barrier(Ok(()));
            recording.fail(operation, occurrence);
            fixture.comb.disk.set_durability(recording);
            let error = fixture.apply().unwrap_err();
            assert_eq!(
                error.file_committed,
                fixture.root.path().join(".comb/import.json").exists(),
                "{operation:?}/{occurrence}"
            );
            let mut fixture = fixture.reopen();
            fixture.apply().unwrap();
            assert_eq!(fixture.status().completed, 3);
            for name in ["a", "b", "c"] {
                assert!(
                    fixture
                        .comb
                        .get(&name.parse().unwrap())
                        .unwrap()
                        .page
                        .timeline
                        .is_empty()
                );
            }
        }
    }
}

#[test]
fn abandonment_failures_and_lost_replies_are_retried_by_exact_id() {
    for (position, step) in STEPS.into_iter().enumerate() {
        let mut fixture = Fixture::new();
        let pending = fixture.pending(1);
        fs::remove_dir_all(fixture.source.path()).unwrap();
        let replacing = Cell::new(false);
        let error = hooks::with(
            move |path, at| {
                if path.file_name().unwrap() == "import.json" && at == Step::Written {
                    replacing.set(true);
                }
                if replacing.get() && path.file_name().unwrap() == "import.json" && at == step {
                    return Err(io::Error::from_raw_os_error(libc::EIO));
                }
                Ok(())
            },
            || fixture.comb.abandon_import(&pending.batch_id).unwrap_err(),
        );
        assert_eq!(error.file_committed, position >= 4, "{step:?}");
        let fixture = fixture.reopen();
        let result = fixture.comb.abandon_import(&pending.batch_id).unwrap();
        assert_eq!((result.completed, result.unfinished), (1, 2));
        assert_eq!(result.state, ImportState::Abandoned);
        assert_eq!(
            fixture.comb.abandon_import(&pending.batch_id).unwrap(),
            result
        );
        assert!(fixture.page("a").exists());
        assert!(!fixture.page("b").exists());
    }
}

#[test]
fn unavailable_progress_never_replaces_the_original_import_failure() {
    use std::error::Error as _;
    let mut fixture = Fixture::new();
    fixture.pending(1);
    let record = fixture.comb.disk.import_record().unwrap().unwrap();
    let journal = fixture.journal();
    let receipt = fs::read_dir(fixture.root.path().join(".comb/receipts"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::write(&receipt, b"corrupt receipt").unwrap();
    let primary = Error::from(io::Error::from_raw_os_error(libc::ENOSPC));
    let failed = record.failure(primary, &fixture.comb.disk);
    assert!(failed.file_committed);
    assert!(failed.import_progress.is_none());
    assert_eq!(
        failed
            .source()
            .unwrap()
            .downcast_ref::<io::Error>()
            .unwrap()
            .raw_os_error(),
        Some(libc::ENOSPC)
    );
    fs::remove_dir_all(fixture.source.path()).unwrap();
    let failed = fixture.apply().unwrap_err();
    assert!(failed.file_committed);
    assert!(failed.import_progress.is_none());
    assert_eq!(
        failed
            .source()
            .unwrap()
            .downcast_ref::<io::Error>()
            .unwrap()
            .kind(),
        io::ErrorKind::NotFound
    );
    assert_eq!(fixture.journal(), journal);
    assert_eq!(fs::read(receipt).unwrap(), b"corrupt receipt");
    assert!(!fixture.page("b").exists());
}
