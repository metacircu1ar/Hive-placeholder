use super::*;
use atomic_fs::Step;
use std::{error::Error as _, io, os::unix::fs::MetadataExt};

fn plan(root: &Path) -> Plan {
    Plan::inspect(root, None, &Options::default()).unwrap()
}

#[test]
fn accepted_intent_retry_repeats_file_and_parent_barriers_without_rewriting() {
    let temp = tempfile::tempdir().unwrap();
    let plan = plan(&temp.path().join("root"));
    let path = plan.root.join(".comb-setup.json");
    let guard = files::hooks::install(|_, at| {
        if at == Step::BeforeDirectorySync {
            Err(io::Error::from_raw_os_error(libc::EIO))
        } else {
            Ok(())
        }
    });
    let error = intent::publish(&plan).unwrap_err();
    assert!(error.file_committed);
    let inode = fs::metadata(&path).unwrap().ino();
    let error = intent::publish(&plan).unwrap_err();
    assert!(error.file_committed);
    let evidence = error
        .source()
        .unwrap()
        .downcast_ref::<atomic_fs::Error>()
        .unwrap();
    assert!(evidence.published());
    assert_eq!(evidence.io_error().raw_os_error(), Some(libc::EIO));
    drop(guard);
    let guard = files::hooks::install(|_, at| {
        if at == Step::BeforeFileSync {
            Err(io::Error::from_raw_os_error(libc::ENOSPC))
        } else {
            Ok(())
        }
    });
    assert!(intent::publish(&plan).unwrap_err().file_committed);
    drop(guard);
    intent::publish(&plan).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
    let mut conflict = plan.clone();
    conflict.socket = conflict.root.join("other.sock");
    assert!(intent::publish(&conflict).is_err());
}

#[test]
fn semantic_intent_retry_accepts_other_json_whitespace_and_absent_clear_retries_sync() {
    let temp = tempfile::tempdir().unwrap();
    let plan = plan(&temp.path().join("root"));
    intent::publish(&plan).unwrap();
    let path = plan.root.join(".comb-setup.json");
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    intent::publish(&plan).unwrap();
    let guard = files::hooks::install(|_, at| {
        if at == Step::BeforeDirectorySync {
            Err(io::Error::from_raw_os_error(libc::EIO))
        } else {
            Ok(())
        }
    });
    assert!(intent::clear(&plan.root).unwrap_err().file_committed);
    assert!(!path.exists());
    assert!(intent::clear(&plan.root).unwrap_err().file_committed);
    drop(guard);
    intent::clear(&plan.root).unwrap();
}
