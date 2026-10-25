use super::*;
use crate::{index::Index, read::Snapshot};
use std::os::unix::fs::PermissionsExt;

fn snapshot(comb: &Comb) -> Result<Snapshot> {
    Snapshot::pin(
        Index::reader(&comb.disk.index_path(), None)?,
        &comb.publication,
    )
}

fn old(snapshot: &Snapshot, request: &Consolidate) {
    assert_eq!(
        snapshot.index.document(&request.target).unwrap().hash,
        request.target_hash
    );
    assert_eq!(
        snapshot.index.document(&request.note).unwrap().hash,
        request.note_hash
    );
    assert_eq!(snapshot.index.stats().unwrap().inbox, 1);
}

#[test]
fn snapshots_keep_the_whole_old_pair_until_the_whole_new_pair_is_published() {
    for failed in [false, true] {
        let mut fixture = Fixture::new();
        let pinned = snapshot(&fixture.comb).unwrap();
        let publication = fixture.comb.publication.clone();
        let index_path = fixture.comb.disk.index_path();
        let request = fixture.request.clone();
        fixture.comb.before_index = Some(Box::new(move || {
            let reader = Index::reader(&index_path, None).unwrap();
            old(&Snapshot::pin(reader, &publication).unwrap(), &request);
        }));
        if failed {
            let recording = Arc::new(Recording::default());
            recording.fail_finish_once();
            fixture.comb.disk.set_durability(recording);
        }
        let result = fixture.apply();
        old(&pinned, &fixture.request);
        if failed {
            assert!(result.unwrap_err().file_committed);
            assert_eq!(
                snapshot(&fixture.comb).err().unwrap().code,
                ErrorCode::RecoveryRequired
            );
        } else {
            let result = result.unwrap();
            let fresh = snapshot(&fixture.comb).unwrap();
            assert_eq!(
                fresh.index.document(&fixture.request.target).unwrap().hash,
                result.target.hash
            );
            assert_eq!(
                fresh.index.document(&fixture.request.note).unwrap().hash,
                result.note.hash
            );
            assert_eq!(fresh.index.stats().unwrap().inbox, 0);
        }
        drop(pinned);
        drop(fixture.comb);
        let comb = Comb::open(fixture.root.path()).unwrap();
        assert_eq!(snapshot(&comb).unwrap().index.stats().unwrap().inbox, 0);
    }
}

#[test]
fn later_index_admission_failure_retains_accepted_pair_and_committed_status() {
    let mut fixture = Fixture::new();
    let pair = fixture.pending();
    let index = fixture.comb.disk.index_path();
    drop(fixture.comb);
    let permissions = fs::metadata(&index).unwrap().permissions();
    fs::set_permissions(&index, fs::Permissions::from_mode(0o620)).unwrap();
    let error = Comb::open(fixture.root.path()).err().unwrap();
    assert_eq!(error.code, ErrorCode::UnsafePath);
    assert!(error.file_committed);
    assert!(fixture.root.path().join(".comb/intent.json").exists());
    fs::set_permissions(&index, permissions).unwrap();
    fixture.comb = Comb::open(fixture.root.path()).unwrap();
    fixture.recover(Some(pair));
}

#[test]
fn failure_indexing_the_second_page_rolls_back_the_first_page_too() {
    let mut fixture = Fixture::new();
    let pinned = snapshot(&fixture.comb).unwrap();
    let index = fixture.comb.disk.index_path();
    let db = rusqlite::Connection::open(&index).unwrap();
    db.execute_batch("CREATE TRIGGER fail_pair_note BEFORE INSERT ON pages WHEN substr(NEW.slug,1,6)='inbox/' BEGIN SELECT RAISE(ABORT,'injected second-row failure'); END").unwrap();
    let (result, pair) = fixture.capture(|_, _| Ok(()));
    let error = result.unwrap_err();
    assert_eq!(error.code, ErrorCode::IndexError);
    assert!(error.file_committed);
    old(&pinned, &fixture.request);
    // Inspect SQL directly: the public gate correctly refuses a fresh snapshot.
    let hashes: (String, String) = db.query_row(
        "SELECT (SELECT content_hash FROM pages WHERE slug=?1),(SELECT content_hash FROM pages WHERE slug=?2)",
        [&fixture.request.target, &fixture.request.note],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert_eq!(
        hashes,
        (
            fixture.request.target_hash.to_string(),
            fixture.request.note_hash.to_string()
        )
    );
    assert_eq!(
        snapshot(&fixture.comb).err().unwrap().code,
        ErrorCode::RecoveryRequired
    );
    db.execute_batch("DROP TRIGGER fail_pair_note").unwrap();
    drop(db);
    drop(pinned);
    fixture.recover(pair);
}
