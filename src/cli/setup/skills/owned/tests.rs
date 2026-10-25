use super::super::publication::tests::Hooks;
use super::*;
use atomic_fs::Step;
use std::{fs, io as stdio};

fn interrupted(root: &Path, count: usize, padding: usize) -> BTreeMap<PathBuf, Vec<u8>> {
    let old = (0..count)
        .map(|i| {
            (
                PathBuf::from(format!("bundle/old-{i:04}{}", "x".repeat(padding))),
                b"old".to_vec(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let wanted = (0..count)
        .map(|i| {
            (
                PathBuf::from(format!("bundle/new-{i:04}{}", "x".repeat(padding))),
                b"new".to_vec(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    files::directory(&root.join("bundle")).unwrap();
    for (path, bytes) in old.iter().chain(&wanted) {
        fs::write(root.join(path), bytes).unwrap();
    }
    let record = Record {
        files: hashes(&old),
        pending: Some(hashes(&wanted)),
    };
    files::write(&root.join(RECORD), &encode(&record).unwrap(), true).unwrap();
    wanted
}

#[test]
fn same_pending_reuses_bounded_intent_and_progresses_after_interruption() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("skill");
    // Both admitted maps fit; duplicating the present union with pending does not.
    let wanted = interrupted(&root, 300, 210);
    let record = read(&root).unwrap();
    let candidate = Group::inspect(root.clone(), wanted.clone()).unwrap();
    let current = candidate.check(&record).unwrap();
    assert!(
        files::encode(&Record {
            files: current,
            pending: record.pending
        })
        .unwrap()
        .len() as u64
            > files::MAX_RECORD
    );
    let bytes = fs::read(root.join(RECORD)).unwrap();
    let hooks = Hooks::new(|_, step| {
        if step == Step::AfterRemove {
            return Err(stdio::Error::from_raw_os_error(libc::EIO));
        }
        Ok(())
    });
    // A real removal proves the accepted bounded intent was durably reused.
    assert_eq!(candidate.apply().unwrap_err().code(), "state_io");
    assert_eq!(fs::read(root.join(RECORD)).unwrap(), bytes);
    assert!(read(&root).unwrap().pending.is_some());
    drop(hooks);
    assert!(
        Group::inspect(root.clone(), wanted.clone())
            .unwrap()
            .apply()
            .unwrap()
    );
    assert!(!Group::inspect(root, wanted).unwrap().apply().unwrap());
}

#[test]
fn oversized_retarget_preserves_exact_old_record_and_files_before_effects() {
    for (count, padding) in [(1025, 0), (300, 210)] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("skill");
        let mut wanted = interrupted(&root, count, padding);
        // Dense short paths exceed2048 in the rewritten files map; padded paths
        // stay below that map limit but exceed256KiB when pending is added.
        if padding == 0 {
            wanted = BTreeMap::from([(PathBuf::from("SKILL.md"), b"retarget".to_vec())]);
        } else {
            wanted.insert(PathBuf::from("SKILL.md"), b"retarget".to_vec());
        }
        let bytes = fs::read(root.join(RECORD)).unwrap();
        let hooks =
            Hooks::new(|_, _| panic!("No recovery or publication effects before bound rejection"));
        let error = Group::inspect(root.clone(), wanted)
            .unwrap()
            .apply()
            .unwrap_err();
        assert_eq!(error.code(), "config_invalid");
        assert!(
            error
                .message()
                .contains(if padding == 0 { "limits" } else { "256 KiB" })
        );
        drop(hooks);
        assert_eq!(fs::read(root.join(RECORD)).unwrap(), bytes);
        assert_eq!(
            fs::read_dir(root.join("bundle")).unwrap().count(),
            count * 2
        );
        assert!(read(&root).unwrap().pending.is_some());
    }
}
