use super::*;
use std::{
    fs, io,
    os::unix::fs::PermissionsExt,
    sync::atomic::{AtomicBool, Ordering},
};

struct Synthetic(u32, u32);

#[test]
fn page_manifests_use_the_same_large_bounded_reader_and_writer() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("manifest.json");
    let value = serde_json::json!({"pages": "x".repeat(512 * 1024)});
    storage::create_record(&path, &value, &atomic_fs::DefaultBarriers).unwrap();
    assert_eq!(storage::record::<serde_json::Value>(&path).unwrap(), value);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(storage::record::<serde_json::Value>(&path).is_err());
}
impl Synthetic {
    fn validate(root: &Path, number: u32) -> Result<()> {
        let expected = format!("{{\"format\":{number}}}");
        if fs::read_to_string(root.join("format"))? != expected
            || fs::read_to_string(root.join("a.md"))? != format!("alpha-{number}")
            || fs::read_to_string(root.join("b.md"))? != format!("beta-{number}")
        {
            return Err(invalid("Synthetic fixture structure differs."));
        }
        Ok(())
    }
}
impl Step for Synthetic {
    fn from(&self) -> u32 {
        self.0
    }
    fn to(&self) -> u32 {
        self.1
    }
    fn files(&self, _: &Path) -> Result<Vec<PathBuf>> {
        Ok(["a.md", "b.md", "missing/deep/optional.json", "format"]
            .map(PathBuf::from)
            .to_vec())
    }
    fn validate_before(&self, p: &Path) -> Result<()> {
        Self::validate(p, self.0)
    }
    fn validate_after(&self, p: &Path) -> Result<()> {
        Self::validate(p, self.1)
    }
    fn transform(&self, p: &Path) -> Result<()> {
        for (name, value) in [
            ("a.md", format!("alpha-{}", self.1)),
            ("b.md", format!("beta-{}", self.1)),
            ("format", format!("{{\"format\":{}}}", self.1)),
        ] {
            files::write(&p.join(name), value.as_bytes(), true)?;
        }
        Ok(())
    }
}
fn fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    for (name, value) in [
        ("a.md", "alpha-1"),
        ("b.md", "beta-1"),
        ("format", "{\"format\":1}"),
    ] {
        files::write(&temp.path().join(name), value.as_bytes(), false).unwrap();
    }
    temp
}

#[test]
fn synthetic_chain_keeps_exact_backups_and_lists_completed_steps() {
    let root = fixture();
    let root_path = root.path().canonicalize().unwrap();
    let owner = || Ok(());
    let engine = Engine::new(&root_path, &root_path, Path::new("format"), &owner).unwrap();
    assert!(engine.pending().unwrap().is_none());
    assert!(engine.history().unwrap().is_empty());
    assert_eq!(fs::read_dir(&root_path).unwrap().count(), 3);
    let first = engine
        .apply(&Synthetic(1, 2), &atomic_fs::DefaultBarriers)
        .unwrap();
    let second = engine
        .apply(&Synthetic(2, 3), &atomic_fs::DefaultBarriers)
        .unwrap();
    Synthetic::validate(&root_path, 3).unwrap();
    Synthetic::validate(&root_path.join("backups").join(&first.backup), 1).unwrap();
    Synthetic::validate(&root_path.join("backups").join(&second.backup), 2).unwrap();
    assert_eq!(engine.history().unwrap(), vec![first, second]);
    assert!(engine.pending().unwrap().is_none());
}

struct Fail {
    root: PathBuf,
    file: &'static str,
    fired: AtomicBool,
}
impl atomic_fs::Barriers for Fail {
    fn observe(&self, step: atomic_fs::Step) -> Result<()> {
        if step == atomic_fs::Step::AfterPublish
            && self.root.join(PENDING).exists()
            && fs::read_to_string(self.root.join(self.file))?.contains('2')
            && !self.fired.swap(true, Ordering::SeqCst)
        {
            return Err(io::Error::from_raw_os_error(libc::EIO));
        }
        Ok(())
    }
}
#[test]
fn every_partial_commit_recovers_and_intervening_editor_bytes_fence_retries() {
    for target in ["a.md", "b.md", "format"] {
        let temp = fixture();
        let root = temp.path().canonicalize().unwrap();
        let owner = || Ok(());
        let engine = Engine::new(&root, &root, Path::new("format"), &owner).unwrap();
        let failure = Fail {
            root: root.clone(),
            file: target,
            fired: AtomicBool::new(false),
        };
        assert!(engine.apply(&Synthetic(1, 2), &failure).is_err());
        assert!(failure.fired.load(Ordering::SeqCst));
        let receipt = engine.pending().unwrap().unwrap();
        Synthetic::validate(&root.join("backups").join(&receipt.backup), 1).unwrap();
        let current = fs::read(root.join("b.md")).unwrap();
        files::write(&root.join("b.md"), b"external editor note", true).unwrap();
        for _ in 0..2 {
            assert!(
                engine
                    .recover(&Synthetic(1, 2), &atomic_fs::DefaultBarriers)
                    .is_err()
            );
            assert_eq!(
                fs::read(root.join("b.md")).unwrap(),
                b"external editor note"
            );
            assert_eq!(engine.pending().unwrap(), Some(receipt.clone()));
        }
        files::write(&root.join("b.md"), &current, true).unwrap();
        engine
            .recover(&Synthetic(1, 2), &atomic_fs::DefaultBarriers)
            .unwrap();
        Synthetic::validate(&root, 2).unwrap();
        assert_eq!(engine.history().unwrap(), vec![receipt]);
    }
}

#[test]
fn source_corruption_is_refused_before_creating_backup_or_intent() {
    let temp = fixture();
    let root = temp.path().canonicalize().unwrap();
    files::write(&root.join("a.md"), b"unexpected", true).unwrap();
    let owner = || Ok(());
    let engine = Engine::new(&root, &root, Path::new("format"), &owner).unwrap();
    assert!(
        engine
            .apply(&Synthetic(1, 2), &atomic_fs::DefaultBarriers)
            .is_err()
    );
    assert_eq!(fs::read_dir(&root).unwrap().count(), 3);
}

#[test]
fn process_death_recovers_each_publication_without_replacing_ownership_locks() {
    use std::os::unix::fs::MetadataExt;
    for target in ["a.md", "b.md", "format"] {
        let temp = fixture();
        let root = temp.path().canonicalize().unwrap();
        files::write(&root.join("native.lock"), b"", false).unwrap();
        let inode = fs::metadata(root.join("native.lock")).unwrap().ino();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "migration::tests::death_fixture", "--ignored"])
            .env("HIVE_MIGRATION_TEST_ROOT", &root)
            .env("HIVE_MIGRATION_TEST_TARGET", target)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(71));
        let owner = || Ok(());
        let engine = Engine::new(&root, &root, Path::new("format"), &owner).unwrap();
        let receipt = engine.pending().unwrap().unwrap();
        Synthetic::validate(&root.join("backups").join(&receipt.backup), 1).unwrap();
        engine
            .recover(&Synthetic(1, 2), &atomic_fs::DefaultBarriers)
            .unwrap();
        Synthetic::validate(&root, 2).unwrap();
        assert_eq!(fs::metadata(root.join("native.lock")).unwrap().ino(), inode);
    }
}

#[test]
#[ignore = "owned subprocess crash fixture"]
fn death_fixture() {
    struct Die(PathBuf, String);
    impl atomic_fs::Barriers for Die {
        fn observe(&self, step: atomic_fs::Step) -> Result<()> {
            if step == atomic_fs::Step::AfterPublish
                && self.0.join(PENDING).exists()
                && fs::read_to_string(self.0.join(&self.1))?.contains('2')
            {
                std::process::exit(71);
            }
            Ok(())
        }
    }
    let root = PathBuf::from(std::env::var_os("HIVE_MIGRATION_TEST_ROOT").unwrap());
    let target = std::env::var("HIVE_MIGRATION_TEST_TARGET").unwrap();
    let owner = || Ok(());
    Engine::new(&root, &root, Path::new("format"), &owner)
        .unwrap()
        .apply(&Synthetic(1, 2), &Die(root.clone(), target))
        .unwrap();
    panic!("crash boundary was not reached");
}

#[test]
fn intent_rejects_nested_arrays_missing_fields_duplicates_and_invalid_hashes() {
    let temp = fixture();
    let root = temp.path().canonicalize().unwrap();
    let owner = || Ok(());
    let engine = Engine::new(&root, &root, Path::new("format"), &owner).unwrap();
    let failure = Fail {
        root: root.clone(),
        file: "a.md",
        fired: AtomicBool::new(false),
    };
    assert!(engine.apply(&Synthetic(1, 2), &failure).is_err());
    let original: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(PENDING)).unwrap()).unwrap();
    let mut invalids = Vec::new();
    let mut array = original.clone();
    array["files"][0] = serde_json::json!([
        "a.md",
        array["files"][0]["before"],
        array["files"][0]["after"]
    ]);
    invalids.push(array);
    let mut array = original.clone();
    let before = &array["files"][0]["before"];
    let sequence = serde_json::json!([before["hash"], before["bytes"], before["mode"]]);
    array["files"][0]["before"] = sequence;
    invalids.push(array);
    let mut missing = original.clone();
    missing["files"][0]
        .as_object_mut()
        .unwrap()
        .remove("before");
    invalids.push(missing);
    let mut hash = original.clone();
    hash["files"][0]["before"]["hash"] = "invalid".into();
    invalids.push(hash);
    for value in invalids {
        files::write(
            &root.join(PENDING),
            &serde_json::to_vec(&value).unwrap(),
            true,
        )
        .unwrap();
        assert!(engine.pending().is_err());
    }
    let duplicate = original
        .to_string()
        .replace("\"mode\":", "\"mode\":384,\"mode\":");
    files::write(&root.join(PENDING), duplicate.as_bytes(), true).unwrap();
    assert!(engine.pending().is_err());
}

struct TerminalFault {
    root: PathBuf,
    boundary: atomic_fs::Step,
    die: bool,
    fired: AtomicBool,
}
impl TerminalFault {
    fn new(root: &Path, boundary: atomic_fs::Step, die: bool) -> Self {
        Self {
            root: root.to_owned(),
            boundary,
            die,
            fired: AtomicBool::new(false),
        }
    }
}
impl atomic_fs::Barriers for TerminalFault {
    fn observe(&self, step: atomic_fs::Step) -> Result<()> {
        if step == self.boundary
            && !self.root.join(PENDING).exists()
            && fs::read_to_string(self.root.join("format"))? == "{\"format\":2}"
        {
            self.fired.store(true, Ordering::SeqCst);
            if self.die {
                std::process::exit(72);
            }
            return Err(io::Error::from_raw_os_error(libc::EIO));
        }
        Ok(())
    }
}

#[test]
fn removed_intent_requires_a_successful_completion_barrier_before_writes() {
    for boundary in [
        atomic_fs::Step::AfterRemove,
        atomic_fs::Step::BeforeDirectorySync,
        atomic_fs::Step::AfterDirectorySync,
    ] {
        let temp = fixture();
        let root = temp.path().canonicalize().unwrap();
        let owner = || Ok(());
        let engine = Engine::new(&root, &root, Path::new("format"), &owner).unwrap();
        let failure = TerminalFault::new(&root, boundary, false);
        assert!(engine.apply(&Synthetic(1, 2), &failure).is_err());
        assert!(failure.fired.load(Ordering::SeqCst));
        assert!(engine.pending().unwrap().is_none());
        assert_eq!(engine.history().unwrap().len(), 1);
        let retry = TerminalFault::new(&root, atomic_fs::Step::BeforeDirectorySync, false);
        for _ in 0..2 {
            assert!(admit_writes(&root, &owner, &retry).is_err());
            assert!(engine.recover(&Synthetic(1, 2), &retry).is_err());
            assert!(retry.fired.load(Ordering::SeqCst));
        }
        files::write(&root.join("a.md"), b"later editor work", true).unwrap();
        assert!(
            engine
                .recover(&Synthetic(1, 2), &atomic_fs::DefaultBarriers)
                .is_err()
        );
        assert_eq!(fs::read(root.join("a.md")).unwrap(), b"later editor work");
        files::write(&root.join("a.md"), b"alpha-2", true).unwrap();
        let receipt = engine
            .recover(&Synthetic(1, 2), &atomic_fs::DefaultBarriers)
            .unwrap();
        assert_eq!(engine.history().unwrap(), vec![receipt]);
        admit_writes(&root, &owner, &atomic_fs::DefaultBarriers).unwrap();
        engine
            .apply(&Synthetic(2, 3), &atomic_fs::DefaultBarriers)
            .unwrap();
    }
}

#[test]
fn process_death_during_completion_retries_the_absent_intent_barrier() {
    for boundary in ["remove", "before-sync", "after-sync"] {
        let temp = fixture();
        let root = temp.path().canonicalize().unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "migration::tests::completion_death_fixture",
                "--ignored",
            ])
            .env("HIVE_MIGRATION_TEST_ROOT", &root)
            .env("HIVE_MIGRATION_TEST_BOUNDARY", boundary)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(72));
        let owner = || Ok(());
        let engine = Engine::new(&root, &root, Path::new("format"), &owner).unwrap();
        assert!(engine.pending().unwrap().is_none());
        let failure = TerminalFault::new(&root, atomic_fs::Step::BeforeDirectorySync, false);
        assert!(admit_writes(&root, &owner, &failure).is_err());
        assert!(failure.fired.load(Ordering::SeqCst));
        engine
            .recover(&Synthetic(1, 2), &atomic_fs::DefaultBarriers)
            .unwrap();
        admit_writes(&root, &owner, &atomic_fs::DefaultBarriers).unwrap();
        Synthetic::validate(&root, 2).unwrap();
    }
}

#[test]
#[ignore = "owned subprocess completion crash fixture"]
fn completion_death_fixture() {
    let root = PathBuf::from(std::env::var_os("HIVE_MIGRATION_TEST_ROOT").unwrap());
    let boundary = match std::env::var("HIVE_MIGRATION_TEST_BOUNDARY")
        .unwrap()
        .as_str()
    {
        "remove" => atomic_fs::Step::AfterRemove,
        "before-sync" => atomic_fs::Step::BeforeDirectorySync,
        "after-sync" => atomic_fs::Step::AfterDirectorySync,
        _ => panic!("unknown owned crash boundary"),
    };
    let owner = || Ok(());
    Engine::new(&root, &root, Path::new("format"), &owner)
        .unwrap()
        .apply(&Synthetic(1, 2), &TerminalFault::new(&root, boundary, true))
        .unwrap();
    panic!("completion crash boundary was not reached");
}

#[test]
fn absent_nested_destinations_retry_the_existing_ancestor_barrier() {
    use std::sync::atomic::AtomicUsize;
    struct FourthDirectory(AtomicUsize);
    impl atomic_fs::Barriers for FourthDirectory {
        fn sync_directory(&self, file: &fs::File) -> Result<()> {
            if self.0.fetch_add(1, Ordering::SeqCst) == 3 {
                return Err(io::Error::from_raw_os_error(libc::EIO));
            }
            file.sync_all()
        }
    }
    let temp = fixture();
    let root = temp.path().canonicalize().unwrap();
    let owner = || Ok(());
    let engine = Engine::new(&root, &root, Path::new("format"), &owner).unwrap();
    let failure = Fail {
        root: root.clone(),
        file: "a.md",
        fired: AtomicBool::new(false),
    };
    assert!(engine.apply(&Synthetic(1, 2), &failure).is_err());
    for _ in 0..2 {
        let failure = FourthDirectory(AtomicUsize::new(0));
        let error = engine.recover(&Synthetic(1, 2), &failure).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EIO));
        assert_eq!(failure.0.load(Ordering::SeqCst), 4);
        assert!(!root.join("missing").exists());
        assert!(engine.pending().unwrap().is_some());
        assert_eq!(
            fs::read_to_string(root.join("format")).unwrap(),
            "{\"format\":1}"
        );
    }
    engine
        .recover(&Synthetic(1, 2), &atomic_fs::DefaultBarriers)
        .unwrap();
    assert!(!root.join("missing").exists());
    Synthetic::validate(&root, 2).unwrap();
}
