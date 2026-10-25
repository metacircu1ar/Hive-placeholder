use super::*;

#[test]
fn unsafe_paths_and_second_writer_are_rejected() {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    assert_eq!(
        Comb::open(root.path()).err().unwrap().code,
        crate::ErrorCode::CombBusy
    );
    assert_eq!(
        "../escape".parse::<Slug>().unwrap_err().code,
        crate::ErrorCode::InvalidSlug
    );
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("linked")).unwrap();
    assert_eq!(
        comb.put_markdown(
            &"linked/escape".parse().unwrap(),
            &markdown("bad"),
            None,
            &fixture_write(&Actor::Human, "link", None).unwrap()
        )
        .unwrap_err()
        .code,
        crate::ErrorCode::UnsafePath
    );
    assert!(!outside.path().join("escape.md").exists());
}

#[test]
fn changed_derived_schema_rebuilds_automatically_and_preserves_knowledge() {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    comb.put_markdown(
        &"concepts/io".parse().unwrap(),
        &markdown("Durable"),
        None,
        &fixture_write(&Actor::Human, "put", None).unwrap(),
    )
    .unwrap();
    comb.index.add_unexpected_table().unwrap();
    drop(comb);
    let comb = Comb::open(root.path()).unwrap();
    assert!(
        !comb
            .get(&"concepts/io".parse().unwrap())
            .unwrap()
            .unverified
    );
}

#[test]
fn missing_identity_and_symlink_database_do_not_create_new_authority() {
    let (root, comb) = TestComb::new().unwrap().into_parts();
    drop(comb);
    let identity = root.path().join(".comb/identity.json");
    std::fs::remove_file(&identity).unwrap();
    assert!(Comb::init(root.path()).is_err());
    assert!(!identity.exists());
    let (root, comb) = TestComb::new().unwrap().into_parts();
    drop(comb);
    let outside = tempfile::NamedTempFile::new().unwrap();
    let index = root.path().join(".comb/index.sqlite");
    std::fs::remove_file(&index).unwrap();
    std::os::unix::fs::symlink(outside.path(), &index).unwrap();
    assert_eq!(
        Comb::open(root.path()).err().unwrap().code,
        crate::ErrorCode::UnsafePath
    );
    assert_eq!(outside.as_file().metadata().unwrap().len(), 0);
}

#[test]
fn open_and_get_do_not_scan_or_parse_unrelated_pages() {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    comb.put_markdown(
        &"concepts/valid".parse().unwrap(),
        &markdown("Valid"),
        None,
        &fixture_write(&Actor::Human, "create", None).unwrap(),
    )
    .unwrap();
    for i in 0..1000 {
        std::fs::write(
            root.path().join(format!("concepts/unrelated-{i}.md")),
            "Not valid frontmatter",
        )
        .unwrap();
    }
    drop(comb);
    let mut comb = Comb::open(root.path()).unwrap();
    assert_eq!(comb.stats().unwrap().quarantined_pages, 0);
    assert_eq!(
        comb.get(&"concepts/valid".parse().unwrap())
            .unwrap()
            .page
            .compiled,
        "Valid"
    );
    assert_eq!(
        comb.doctor()
            .unwrap()
            .findings
            .iter()
            .filter(|f| f.kind == FindingKind::InvalidPage)
            .count(),
        1000
    );
    comb.sync().unwrap();
    assert_eq!(comb.stats().unwrap().pages, 1);
    assert_eq!(comb.stats().unwrap().quarantined_pages, 1000);
    for i in 0..1000 {
        assert_eq!(
            std::fs::read(root.path().join(format!("concepts/unrelated-{i}.md"))).unwrap(),
            b"Not valid frontmatter"
        );
    }
}

#[test]
fn lowercase_slugs_prevent_filesystem_case_aliases_and_import_normalizes_links() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    assert_eq!(
        "components/Writer".parse::<Slug>().unwrap_err().code,
        crate::ErrorCode::InvalidSlug
    );
    let source = tempfile::tempdir().unwrap();
    std::fs::write(
        source.path().join("README.md"),
        "# Guide\n\n[Other](Details.md)",
    )
    .unwrap();
    std::fs::write(source.path().join("Details.md"), "# Details\n\nReference").unwrap();
    comb.import(source.path(), Some(&"operations".parse().unwrap()))
        .unwrap();
    assert_eq!(
        comb.index()
            .unwrap()
            .links(&"operations/readme".parse().unwrap(), false)
            .unwrap(),
        vec!["operations/details".parse::<Slug>().unwrap()]
    );
    assert_eq!(
        "operations/README".parse::<Slug>().unwrap_err().code,
        crate::ErrorCode::InvalidSlug
    );
}

#[test]
fn mis_cased_existing_directories_are_not_used_for_new_pages() {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    std::fs::create_dir(root.path().join("Components")).unwrap();
    // On case-sensitive filesystems this is a distinct path, not an alias.
    if !root.path().join("components").exists() {
        return;
    }
    let error = comb
        .put_markdown(
            &"components/new".parse().unwrap(),
            &markdown("New"),
            None,
            &fixture_write(&Actor::Human, "new", None).unwrap(),
        )
        .unwrap_err();
    assert_eq!(error.code, crate::ErrorCode::UnsafePath);
    assert!(!root.path().join("Components/new.md").exists());
}
