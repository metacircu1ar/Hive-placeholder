use super::*;
use std::{cell::Cell, sync::mpsc::sync_channel, time::Duration};

#[test]
fn coordinated_reader_sees_invalidation_before_any_output_disappears() {
    let temporary = crate::runtime::test_directory();
    let root = temporary.path();
    let store = seed(root);
    let (phase, phases) = sync_channel(1);
    let (observed, observations) = sync_channel(1);
    std::thread::scope(|scope| {
        let reader = scope.spawn(move || {
            for expected in [0, 4, 9] {
                assert_eq!(
                    phases.recv_timeout(Duration::from_secs(10)).unwrap(),
                    expected
                );
                if expected == 0 {
                    assert_eq!(fs::read(manifest(root)).unwrap(), b"derived manifest\n");
                } else {
                    assert!(!manifest(root).exists());
                }
                for id in IDS {
                    assert_eq!(output(root, id).exists(), expected < 9);
                }
                assert_eq!(fs::read(output(root, PEER)).unwrap(), b"{\"keep\":true}\n");
                observed.send(()).unwrap();
            }
        });
        let count = Cell::new(0);
        with_hook(
            move |step| {
                let index = count.get();
                assert_eq!(step, STEPS[index]);
                count.set(index + 1);
                if [0, 4, 9].contains(&index) {
                    phase.send(index).map_err(io::Error::other)?;
                    observations
                        .recv_timeout(Duration::from_secs(10))
                        .map_err(io::Error::other)?;
                }
                Ok(())
            },
            || store.finish_artifact_deletions(),
        )
        .unwrap();
        reader.join().unwrap();
    });
    assert!(store.pending_artifact_deletions().unwrap().ids.is_empty());
    peer_unchanged(&store);
}
