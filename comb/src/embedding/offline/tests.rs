use super::*;
use crate::{Actor, ErrorCode, store::Write, test_support::assert_code};
use std::{error::Error as _, fs, path::PathBuf};

fn space() -> Space {
    Space {
        backend: "mock".into(),
        model: "PRIVATE_MODEL".into(),
        revision: "pin".into(),
        dimensions: 2,
        document_prefix: String::new(),
        query_prefix: String::new(),
        preprocessing: "mock".into(),
        normalization: "l2".into(),
    }
}

#[test]
fn pending_rebuild_and_population_handoff_refuse_offline_inspection_without_recovery() {
    for handoff in [false, true] {
        let (root, space) = fixture(true).unwrap();
        let count = std::cell::Cell::new(0);
        let error = crate::disk::publication_hooks::with(
            move |path, step| {
                let name = if handoff {
                    "index-population.json"
                } else {
                    "index-rebuild.json"
                };
                let boundary = if handoff {
                    atomic_fs::Step::BeforeFileSync
                } else {
                    atomic_fs::Step::AfterPublish
                };
                if path.file_name().unwrap() == name && step == boundary {
                    count.set(count.get() + 1);
                    if count.get() == if handoff { 3 } else { 1 } {
                        return Err(std::io::Error::from_raw_os_error(libc::EIO));
                    }
                }
                Ok(())
            },
            || Comb::rebuild(root.path()),
        )
        .err()
        .unwrap();
        assert!(error.file_committed);
        let names = [
            "index-rebuild.json",
            "index-population.json",
            "index.sqlite",
            "vectors.sqlite",
        ];
        let before: Vec<_> = names
            .iter()
            .map(|name| crate::test_support::optional_bytes(root.path().join(".comb").join(name)))
            .collect();
        for replace in [false, true] {
            assert_code!(
                Comb::open_for_embedding(root.path(), &space, replace),
                ErrorCode::RecoveryRequired
            );
        }
        let after: Vec<_> = names
            .iter()
            .map(|name| crate::test_support::optional_bytes(root.path().join(".comb").join(name)))
            .collect();
        assert_eq!(before, after);
        drop(Comb::open(root.path()).unwrap());
        assert!(!root.path().join(".comb/index-rebuild.json").exists());
    }
}
fn fixture(active: bool) -> Result<(tempfile::TempDir, Space)> {
    let root = tempfile::tempdir()?;
    let mut comb = Comb::init(root.path())?;
    let space = space();
    if active {
        comb.activate_embeddings(&space, false)?;
    }
    drop(comb);
    Ok((root, space))
}
fn input_failure(root: &Path, space: &Space, replace: bool, code: Code) -> Error {
    let error = assert_code!(Comb::open_for_embedding(root, space, replace), code.into());
    let input = error.input().expect("typed replacement error");
    assert_eq!(input.field(), "/options/replace");
    assert!(!input.examples().is_empty());
    assert!(!error.file_committed);
    assert!(
        !serde_json::to_string(&error)
            .expect("wire error")
            .contains("PRIVATE_MODEL")
    );
    error
}
#[test]
fn strict_flags_preserve_native_idempotence_and_hold_the_root_guard() -> Result<()> {
    let (root, space) = fixture(false)?;
    input_failure(root.path(), &space, true, Code::ConflictingArguments);
    let mut comb = Comb::open_for_embedding(root.path(), &space, false)?;
    assert_code!(Comb::open(root.path()), ErrorCode::CombBusy);
    assert_code!(
        Comb::open_for_embedding(root.path(), &space, false),
        ErrorCode::CombBusy
    );
    let lease = comb.activate_embeddings(&space, false)?;
    assert_eq!(comb.activate_embeddings(&space, true)?, lease);
    drop(comb);
    input_failure(root.path(), &space, true, Code::ConflictingArguments);
    let mut changed = space.clone();
    changed.revision = "other".into();
    let error = input_failure(root.path(), &changed, false, Code::MissingArgument);
    assert_eq!(
        error
            .source()
            .and_then(|s| s.downcast_ref::<Error>())
            .map(|e| e.code),
        Some(ErrorCode::EmbeddingReplaceRequired)
    );
    let mut comb = Comb::open_for_embedding(root.path(), &changed, true)?;
    assert_code!(Comb::open(root.path()), ErrorCode::CombBusy);
    let replacement = comb.activate_embeddings(&changed, true)?;
    assert_ne!(replacement, lease);
    assert_eq!(comb.index()?.embedding_lease(&changed)?, replacement);
    Ok(())
}
fn pending(root: &Path, comb: &mut Comb) -> Result<Vec<(PathBuf, Vec<u8>)>> {
    let slug = "concepts/pending".parse()?;
    comb.put_markdown(
        &slug,
        "---\ntitle: Pending\n---\nBefore",
        None,
        &Write::new(&Actor::Human, &"create".parse()?, None)?,
    )?;
    let loaded = comb.get(&slug)?;
    let mut page = loaded.page;
    page.compiled = "After crash".into();
    comb.disk.seal(&slug, &mut page)?;
    comb.disk
        .write(&slug, Some(&page), Some(&loaded.hash), None)?;
    comb.disk.index_population(true)?;
    [
        "concepts/pending.md",
        ".comb/intent.json",
        ".comb/index-population.json",
    ]
    .into_iter()
    .map(|name| {
        let path = root.join(name);
        Ok((path.clone(), fs::read(path)?))
    })
    .collect()
}
#[test]
fn rejected_replacement_preserves_pending_page_and_population_before_recovery() -> Result<()> {
    let (root, space) = fixture(true)?;
    let mut comb = Comb::open(root.path())?;
    let retained = pending(root.path(), &mut comb)?;
    drop(comb);
    let index = root.path().join(".comb/index.sqlite");
    let before = fs::read(&index)?;
    input_failure(root.path(), &space, true, Code::ConflictingArguments);
    assert_eq!(fs::read(index)?, before);
    for (path, bytes) in retained {
        assert_eq!(fs::read(path)?, bytes);
    }
    let comb = Comb::open_for_embedding(root.path(), &space, false)?;
    assert!(!root.path().join(".comb/intent.json").exists());
    assert!(!root.path().join(".comb/index-population.json").exists());
    assert_eq!(
        comb.get(&"concepts/pending".parse()?)?.page.compiled,
        "After crash"
    );
    Ok(())
}
#[test]
fn schema_and_selection_corruption_remain_operational_before_flag_errors() -> Result<()> {
    for (name, sql, expected) in [
        (
            "index.sqlite",
            "CREATE TABLE unexpected(x TEXT)",
            ErrorCode::UnsupportedIndex,
        ),
        (
            "vectors.sqlite",
            "CREATE TABLE unexpected(x TEXT)",
            ErrorCode::UnsupportedIndex,
        ),
        (
            "index.sqlite",
            "UPDATE embedding_space SET specification='invalid'",
            ErrorCode::InvalidJson,
        ),
        (
            "index.sqlite",
            "UPDATE embedding_space SET fingerprint='invalid'",
            ErrorCode::InvalidEmbeddingState,
        ),
    ] {
        let (root, space) = fixture(true)?;
        let path = root.path().join(".comb").join(name);
        let connection = rusqlite::Connection::open(&path)?;
        connection.execute_batch(sql)?;
        drop(connection);
        let before = fs::read(&path)?;
        let error = assert_code!(
            Comb::open_for_embedding(root.path(), &space, true),
            expected
        );
        assert!(error.input().is_none());
        assert_eq!(fs::read(path)?, before);
    }
    Ok(())
}
#[test]
fn missing_databases_are_not_created_by_rejected_flags_and_valid_requests_rebuild() -> Result<()> {
    for name in ["index.sqlite", "vectors.sqlite"] {
        let (root, space) = fixture(false)?;
        let path = root.path().join(".comb").join(name);
        fs::remove_file(&path)?;
        input_failure(root.path(), &space, true, Code::ConflictingArguments);
        assert!(!path.exists());
        drop(Comb::open_for_embedding(root.path(), &space, false)?);
        assert!(path.exists());
    }
    Ok(())
}
#[test]
fn unsafe_vector_metadata_is_not_hidden_by_a_redundant_flag() -> Result<()> {
    let (root, space) = fixture(true)?;
    let path = root.path().join(".comb/vectors.sqlite");
    let original = path.with_extension("retained");
    fs::rename(&path, &original)?;
    std::os::unix::fs::symlink(original, path)?;
    let error = assert_code!(
        Comb::open_for_embedding(root.path(), &space, true),
        ErrorCode::UnsafePath
    );
    assert!(error.input().is_none());
    Ok(())
}
