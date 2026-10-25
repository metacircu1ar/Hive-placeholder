use super::*;

#[test]
fn each_pair_publication_failure_preserves_exact_prepared_pages_and_both_replays() {
    for slot in 0..7 {
        for (position, step) in STEPS.into_iter().enumerate() {
            let mut fixture = Fixture::new();
            let selected = fixture.paths()[slot].clone();
            let (result, pair) = fixture.capture(move |path, at| {
                if path == selected && at == step {
                    return Err(io::Error::from_raw_os_error(libc::EIO));
                }
                Ok(())
            });
            let error = result.unwrap_err();
            let accepted = slot != 0 || position >= 4;
            assert_eq!(error.file_committed, accepted, "{slot}/{step:?}");
            assert_eq!(pair.is_some(), accepted);
            if accepted {
                assert_eq!(
                    fixture.comb.ready().unwrap_err().code,
                    ErrorCode::RecoveryRequired
                );
            } else {
                fixture.comb.ready().unwrap();
                assert_eq!(
                    fixture.comb.get(&fixture.request.target).unwrap().hash,
                    fixture.request.target_hash
                );
                assert_eq!(
                    fixture.comb.get(&fixture.request.note).unwrap().hash,
                    fixture.request.note_hash
                );
            }
            fixture.recover(pair);
        }
    }
}

#[test]
fn every_native_barrier_failure_keeps_the_acceptance_boundary_and_recoverable_pair() {
    let mut fixture = Fixture::new();
    let recording = Arc::new(Recording::default());
    recording.barrier(Ok(()));
    fixture.comb.disk.set_durability(recording.clone());
    fixture.apply().unwrap();
    let events = recording.counts().events;
    for operation in [
        Operation::Fsync,
        Operation::FullFlush,
        Operation::BeforeFinish,
    ] {
        for occurrence in 1..=events.iter().filter(|op| **op == operation).count() {
            let mut fixture = Fixture::new();
            let recording = Arc::new(Recording::default());
            recording.barrier(Ok(()));
            recording.fail(operation, occurrence);
            fixture.comb.disk.set_durability(recording);
            let (result, pair) = fixture.capture(|_, _| Ok(()));
            assert_eq!(
                result.unwrap_err().file_committed,
                pair.is_some(),
                "{operation:?}/{occurrence}"
            );
            fixture.recover(pair);
        }
    }
}

#[test]
fn retry_repeats_existing_intent_page_and_receipt_barriers() {
    for slot in [0, 1, 3, 4, 6] {
        for step in [
            Step::BeforeFileSync,
            Step::AfterFileSync,
            Step::BeforeDirectorySync,
            Step::AfterDirectorySync,
        ] {
            let mut fixture = Fixture::new();
            let recording = Arc::new(Recording::default());
            recording.fail_finish_once();
            fixture.comb.disk.set_durability(recording);
            let (result, pair) = fixture.capture(|_, _| Ok(()));
            assert!(result.unwrap_err().file_committed);
            let selected = fixture.paths()[slot].clone();
            let root = fixture.root.path().to_owned();
            drop(fixture.comb);
            let error = hooks::with(
                move |path, at| {
                    if path == selected && at == step {
                        return Err(io::Error::from_raw_os_error(libc::EIO));
                    }
                    Ok(())
                },
                || Comb::open(&root),
            )
            .err()
            .unwrap();
            assert!(error.file_committed);
            assert!(root.join(".comb/intent.json").exists());
            fixture.comb = Comb::open(&root).unwrap();
            fixture.recover(pair);
        }
    }
}

#[test]
fn editor_change_at_the_final_page_check_is_preserved_and_fences_recovery() {
    for slot in [1, 4] {
        let mut fixture = Fixture::new();
        let selected = fixture.paths()[slot].clone();
        let edit_path = selected.clone();
        let changed = std::cell::Cell::new(false);
        let edited = b"---\ntitle: Editor\n---\nIndependent edit";
        let (result, _) = fixture.capture(move |path, step| {
            if path == edit_path && step == Step::BeforePublish && !changed.replace(true) {
                atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
                    .write(path, edited)
                    .unwrap();
            }
            Ok(())
        });
        let error = result.unwrap_err();
        assert!(error.file_committed);
        assert_eq!(error.code, ErrorCode::RecoveryRequired);
        assert_eq!(fs::read(&selected).unwrap(), edited);
        drop(fixture.comb);
        assert!(
            Comb::open(fixture.root.path())
                .err()
                .unwrap()
                .file_committed
        );
        assert_eq!(fs::read(selected).unwrap(), edited);
    }
}

#[test]
fn both_pages_repeat_source_directory_barrier_before_finishing_recovery() {
    for slot in [1, 4] {
        for step in [
            Step::BeforeSourceDirectorySync,
            Step::AfterSourceDirectorySync,
        ] {
            let mut fixture = Fixture::new();
            let recording = Arc::new(Recording::default());
            recording.fail_finish_once();
            fixture.comb.disk.set_durability(recording);
            let (result, pair) = fixture.capture(|_, _| Ok(()));
            assert!(result.unwrap_err().file_committed);
            let selected = fixture.paths()[slot].clone();
            let root = fixture.root.path().to_owned();
            drop(fixture.comb);
            for _ in 0..2 {
                let selected = selected.clone();
                let error = hooks::with(
                    move |path, at| {
                        if path == selected && at == step {
                            return Err(io::Error::from_raw_os_error(libc::EIO));
                        }
                        Ok(())
                    },
                    || Comb::open(&root),
                )
                .err()
                .unwrap();
                assert!(error.file_committed);
                assert!(root.join(".comb/intent.json").exists());
            }
            fixture.comb = Comb::open(&root).unwrap();
            fixture.recover(pair);
        }
    }
}
