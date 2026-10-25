//! Verify retained controller exports and isolated replacement namespace fences.
use super::*;
use crate::config::{Method, Profile, Provider};
use crate::state::directory;
use std::io::Write;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::PermissionsExt;
fn malformed_evidence(profile: &Path) -> Vec<PathBuf> {
    let bad_receipt = profile.join("executions/corrupt.json");
    super::super::private_file(&bad_receipt, true)
        .unwrap()
        .write_all(b"broken")
        .unwrap();
    let evidence = profile.join("executions");
    directory(&evidence).unwrap();
    let stray = evidence.join("unreadable.json");
    super::super::private_file(&stray, true)
        .unwrap()
        .write_all(b"broken")
        .unwrap();
    let non_utf8 = evidence.join(std::ffi::OsString::from_vec(b"bad-\xff".to_vec()));
    let path = export_path(&non_utf8);
    let encoded = serde_json::to_value((path.diagnostic, path.bytes)).unwrap();
    assert!(encoded[0].as_str().unwrap().ends_with("bad-\u{fffd}"));
    assert_eq!(encoded[1].as_array().unwrap().last().unwrap(), 255);
    let non_utf8 = if cfg!(target_os = "linux") {
        super::super::private_file(&non_utf8, true)
            .unwrap()
            .write_all(b"broken")
            .unwrap();
        Some(non_utf8)
    } else {
        None // APFS rejects this filename; encoding is still checked above.
    };
    let mut files = vec![bad_receipt, stray];
    files.extend(non_utf8);
    files
}
#[test]
fn replacement_and_wal_export_never_lock_modify_or_adopt_old_controller() {
    let tmp = tempfile::tempdir().unwrap();
    let old = State::new(tmp.path().join("old")).unwrap();
    let binding = old
        .register(
            "main",
            &Profile {
                provider: Provider::Codex,
                method: Method::Subscription,
                api_key_env: None,
                api_key_file: None,
            },
        )
        .unwrap();
    let receipt_dir = binding.directory.join("executions");
    directory(&receipt_dir).unwrap();
    let receipt = receipt_dir.join("receipt.json");
    write_json(
        &receipt,
        &serde_json::json!({"confirmed":false,"phase":"authorized"}),
    )
    .unwrap();
    let malformed = malformed_evidence(&binding.directory);
    write_json(
        &binding.directory.join("provider/auth.json"),
        &serde_json::json!({"token":"must-not-export"}),
    )
    .unwrap();
    let (path, _db) = wal_export_fixture(&old);
    let mut files = vec![
        path.clone(),
        old.root.join("runtime/runtime.sqlite-wal"),
        old.root.join("runtime/runtime.sqlite-shm"),
        old.root.join("index.json"),
        old.root.join("index.lock"),
        receipt,
    ];
    files.extend(malformed);
    let before = files
        .iter()
        .map(|p| fs::read(p).unwrap())
        .collect::<Vec<_>>();
    let fresh = old
        .replacement_root(&tmp.path().join("fresh"), Provider::Codex)
        .unwrap();
    assert!(!fresh.root.join("index.json").exists());
    assert!(!fresh.root.join("runtime").exists());
    let output = tmp.path().join("export.ndjson");
    let report = fresh.export_retired(&output).unwrap();
    assert_eq!(report["rows"], 4);
    assert_eq!(
        report["ownership_evidence"]["unavailable"],
        2 + u64::from(cfg!(target_os = "linux"))
    );
    let exported = fs::read_to_string(output).unwrap();
    assert!(exported.contains("uncertain input"));
    assert!(exported.contains("authorized"));
    assert!(exported.contains("00ff"));
    assert!(!exported.contains("must-not-export"));
    for (path, bytes) in files.iter().zip(before) {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    assert!(!old.root.join("runtime/daemon.lock").exists());
    assert!(
        fresh
            .export_retired(&old.root.join("forbidden-export"))
            .is_err()
    );
    assert!(
        old.replacement_root(&old.root.join("nested"), Provider::Codex)
            .is_err()
    );
}

pub(super) fn wal_export_fixture(old: &State) -> (PathBuf, rusqlite::Connection) {
    directory(&old.root).unwrap();
    directory(&old.root.join("runtime")).unwrap();
    let path = old.root.join("runtime/runtime.sqlite");
    drop(super::super::private_file(&path, true).unwrap());
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE agents(data TEXT); CREATE TABLE messages(body TEXT); CREATE TABLE events(data TEXT); INSERT INTO agents VALUES('old conversation'); INSERT INTO messages VALUES('uncertain input'); INSERT INTO events VALUES('saved evidence');").unwrap();
    db.execute("INSERT INTO events VALUES(?1)", [&[0_u8, 255][..]])
        .unwrap();
    // Test-created SQLite companions must meet the real private-state policy.
    for name in ["runtime.sqlite-wal", "runtime.sqlite-shm"] {
        fs::set_permissions(
            old.root.join("runtime").join(name),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    (path, db)
}
