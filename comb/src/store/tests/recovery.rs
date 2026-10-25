use super::*;
use std::fs;

#[test]
fn file_commit_before_index_commit_recovers_without_losing_trust() {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    comb.put_markdown(
        &"components/writer".parse().unwrap(),
        &markdown("Before"),
        None,
        &fixture_write(&Actor::Human, "first", None).unwrap(),
    )
    .unwrap();
    let loaded = comb.get(&"components/writer".parse().unwrap()).unwrap();
    let mut page = loaded.page;
    page.compiled = "After crash".into();
    page.promote(Actor::Human).unwrap();
    comb.disk
        .seal(&"components/writer".parse().unwrap(), &mut page)
        .unwrap();
    // Exact crash boundary: durable page/head/intent, old database still present.
    comb.disk
        .write(
            &"components/writer".parse().unwrap(),
            Some(&page),
            Some(&loaded.hash),
            None,
        )
        .unwrap();
    drop(comb);
    let comb = Comb::open(root.path()).unwrap();
    let doc = comb.get(&"components/writer".parse().unwrap()).unwrap();
    assert_eq!(doc.page.compiled, "After crash");
    assert!(!doc.unverified);
    assert_eq!(
        comb.index()
            .unwrap()
            .search(&Search {
                query: "crash".into(),
                ..Search::default()
            })
            .unwrap()
            .results
            .len(),
        1
    );
    assert!(!root.path().join(".comb/intent.json").exists());
}

#[test]
fn partial_scan_does_not_delete_previously_indexed_pages() {
    use std::os::unix::fs::PermissionsExt;
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    comb.put_markdown(
        &"components/first".parse().unwrap(),
        &markdown("First"),
        None,
        &fixture_write(&Actor::Human, "first", None).unwrap(),
    )
    .unwrap();
    comb.put_markdown(
        &"components/second".parse().unwrap(),
        &markdown("Second"),
        None,
        &fixture_write(&Actor::Human, "second", None).unwrap(),
    )
    .unwrap();
    std::fs::remove_file(root.path().join("components/first.md")).unwrap();
    std::fs::write(
        root.path().join("components/second.md"),
        "broken frontmatter",
    )
    .unwrap();
    std::fs::set_permissions(
        root.path().join("components/second.md"),
        std::fs::Permissions::from_mode(0o666),
    )
    .unwrap();
    assert!(comb.sync().is_err());
    assert_eq!(comb.stats().unwrap().pages, 2);
}

#[test]
fn malformed_target_of_accepted_mutation_still_fences_recovery() {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    let slug: Slug = "concepts/pending".parse().unwrap();
    comb.put_markdown(
        &slug,
        &markdown("Original"),
        None,
        &fixture_write(&Actor::Human, "first", None).unwrap(),
    )
    .unwrap();
    let before = comb.get(&slug).unwrap();
    let mut page = before.page.clone();
    page.compiled = "Accepted".into();
    page.promote(Actor::Human).unwrap();
    comb.disk.seal(&slug, &mut page).unwrap();
    comb.disk
        .write(&slug, Some(&page), Some(&before.hash), None)
        .unwrap();
    let intent = root.path().join(".comb/intent.json");
    let retained = fs::read(&intent).unwrap();
    let target = root.path().join("concepts/pending.md");
    fs::write(&target, b"malformed external edit").unwrap();
    fs::write(
        root.path().join("concepts/unrelated.md"),
        b"another malformed page",
    )
    .unwrap();
    drop(comb);
    for _ in 0..2 {
        assert!(Comb::open(root.path()).is_err());
        assert_eq!(fs::read(&intent).unwrap(), retained);
        assert_eq!(fs::read(&target).unwrap(), b"malformed external edit");
    }
}

#[test]
fn pending_intent_before_rename_keeps_old_content_and_receipts() {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    comb.put_markdown(
        &"concepts/io".parse().unwrap(),
        &markdown("Original"),
        None,
        &fixture_write(&Actor::Human, "put", None).unwrap(),
    )
    .unwrap();
    let before = comb.get(&"concepts/io".parse().unwrap()).unwrap();
    crate::disk::atomic(&root.path().join(".comb/intent.json"),&serde_json::to_vec(&serde_json::json!({"operation":"single","record":{"slug":"concepts/io","before":before.hash,"previous_head":before.hash,"after":page::hash("Never renamed"),"receipt":null}})).unwrap(), &crate::disk::System).unwrap();
    drop(comb);
    let mut comb = Comb::open(root.path()).unwrap();
    assert_eq!(
        comb.get(&"concepts/io".parse().unwrap())
            .unwrap()
            .page
            .compiled,
        "Original"
    );
    assert!(
        comb.put_markdown(
            &"concepts/io".parse().unwrap(),
            &markdown("Original"),
            None,
            &fixture_write(&Actor::Human, "put", None).unwrap()
        )
        .unwrap()
        .replayed
    );
}

#[test]
fn recovery_repairs_missing_head_update_after_page_rename() {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    comb.put_markdown(
        &"concepts/io".parse().unwrap(),
        &markdown("Before"),
        None,
        &fixture_write(&Actor::Human, "put", None).unwrap(),
    )
    .unwrap();
    let head = root
        .path()
        .join(".comb/heads")
        .join(format!("{}.json", page::hash("concepts/io")));
    let old_head = std::fs::read(&head).unwrap();
    let loaded = comb.get(&"concepts/io".parse().unwrap()).unwrap();
    let mut page = loaded.page;
    page.compiled = "Renamed".into();
    page.promote(Actor::Human).unwrap();
    comb.disk
        .seal(&"concepts/io".parse().unwrap(), &mut page)
        .unwrap();
    comb.disk
        .write(
            &"concepts/io".parse().unwrap(),
            Some(&page),
            Some(&loaded.hash),
            None,
        )
        .unwrap();
    crate::disk::atomic(&head, &old_head, &crate::disk::System).unwrap();
    drop(comb);
    let comb = Comb::open(root.path()).unwrap();
    let doc = comb.get(&"concepts/io".parse().unwrap()).unwrap();
    assert_eq!(doc.page.compiled, "Renamed");
    assert!(!doc.unverified);
}

#[test]
fn receipt_recovery_finishes_a_committed_page_before_admitting_retry() {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    let actor = agent("fixture", "a", Some("implementor"), "execution", "turn");
    comb.put_markdown(
        &"concepts/io".parse().unwrap(),
        &markdown("Before"),
        None,
        &fixture_write(&actor, "create", None).unwrap(),
    )
    .unwrap();
    // Make index commit fail after the page/receipt were made durable.
    comb.index.reject_page_insert().unwrap();
    let error = comb
        .append(
            &"concepts/io".parse().unwrap(),
            "After failure",
            "test",
            None,
            None,
            &fixture_write(&actor, "append", None).unwrap(),
        )
        .unwrap_err();
    assert!(error.file_committed);
    assert_eq!(
        comb.get(&"concepts/io".parse().unwrap()).unwrap_err().code,
        crate::ErrorCode::RecoveryRequired
    );
    comb.index.allow_page_insert().unwrap();
    remove_appended_receipt(root.path());
    drop(comb);
    let mut comb = Comb::open(root.path()).unwrap();
    assert!(
        comb.append(
            &"concepts/io".parse().unwrap(),
            "After failure",
            "test",
            None,
            None,
            &fixture_write(&actor, "append", None).unwrap()
        )
        .unwrap()
        .replayed
    );
    assert_eq!(
        comb.get(&"concepts/io".parse().unwrap())
            .unwrap()
            .page
            .timeline
            .len(),
        1
    );
}

fn remove_appended_receipt(root: &std::path::Path) {
    // Simulate the earlier crash boundary: page renamed, receipt not yet saved.
    for file in std::fs::read_dir(root.join(".comb/receipts")).unwrap() {
        let file = file.unwrap();
        let receipt: Receipt =
            serde_json::from_slice(&std::fs::read(file.path()).unwrap()).unwrap();
        if receipt.request_id == "append".parse().unwrap() {
            std::fs::remove_file(file.path()).unwrap();
        }
    }
}

#[test]
fn interrupted_population_is_never_served_as_a_complete_index() {
    use std::os::unix::fs::PermissionsExt;
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    comb.put_markdown(
        &"concepts/good".parse().unwrap(),
        &markdown("Good indexed content"),
        None,
        &fixture_write(&Actor::Human, "create", None).unwrap(),
    )
    .unwrap();
    let bad = root.path().join("concepts/bad.md");
    std::fs::write(&bad, "Malformed").unwrap();
    std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o666)).unwrap();
    drop(comb);
    assert!(Comb::rebuild(root.path()).is_err());
    assert!(root.path().join(".comb/index-population.json").exists());
    assert!(Comb::open(root.path()).is_err());
    std::fs::write(&bad, markdown("Repaired content")).unwrap();
    std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o600)).unwrap();
    let comb = Comb::open(root.path()).unwrap();
    assert_eq!(comb.stats().unwrap().pages, 2);
    assert!(!root.path().join(".comb/index-population.json").exists());
    assert_eq!(
        comb.index()
            .unwrap()
            .search(&Search {
                query: "Repaired".into(),
                ..Search::default()
            })
            .unwrap()
            .results[0]
            .slug,
        "concepts/bad".parse().unwrap()
    );
}

#[test]
fn tentative_sidecars_are_retracted_when_the_old_page_survives() {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    let actor = agent("fixture", "a", Some("implementor"), "execution", "turn");
    comb.put_markdown(
        &"concepts/io".parse().unwrap(),
        &markdown("Original"),
        None,
        &fixture_write(&actor, "create", None).unwrap(),
    )
    .unwrap();
    let original = std::fs::read(root.path().join("concepts/io.md")).unwrap();
    comb.index.reject_page_insert().unwrap();
    assert!(
        comb.append(
            &"concepts/io".parse().unwrap(),
            "Pending measurement",
            "test",
            None,
            None,
            &fixture_write(&actor, "append", None).unwrap()
        )
        .unwrap_err()
        .file_committed
    );
    comb.index.allow_page_insert().unwrap();
    // Exercise the conservative recovery branch even if the filesystem/device
    // orders sidecars ahead of the page. No receipt may acknowledge absent work.
    crate::disk::atomic(
        &root.path().join("concepts/io.md"),
        &original,
        &crate::disk::System,
    )
    .unwrap();
    drop(comb);
    let mut comb = Comb::open(root.path()).unwrap();
    assert_retracted_append_can_be_applied(&mut comb, &actor);
}

fn assert_retracted_append_can_be_applied(comb: &mut Comb, actor: &Actor) {
    assert!(
        !comb
            .get(&"concepts/io".parse().unwrap())
            .unwrap()
            .page
            .metadata
            .owned()
            .external()
    );
    let reply = comb
        .append(
            &"concepts/io".parse().unwrap(),
            "Pending measurement",
            "test",
            None,
            None,
            &fixture_write(actor, "append", None).unwrap(),
        )
        .unwrap();
    assert!(!reply.replayed);
    assert_eq!(
        comb.get(&"concepts/io".parse().unwrap())
            .unwrap()
            .page
            .timeline
            .len(),
        1
    );
}

#[test]
fn mutation_coalesces_full_device_flushes_without_dropping_ordering_fallbacks() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    comb.put_markdown(
        &"concepts/io".parse().unwrap(),
        &markdown("Original"),
        None,
        &fixture_write(&Actor::Human, "create", None).unwrap(),
    )
    .unwrap();
    let durability = std::sync::Arc::new(crate::disk::Recording::default());
    comb.disk.set_durability(durability.clone());
    durability.reset();
    comb.append(
        &"concepts/io".parse().unwrap(),
        "Measured",
        "test",
        None,
        None,
        &fixture_write(&Actor::Human, "append", None).unwrap(),
    )
    .unwrap();
    let barriers = durability.counts().full_barriers;
    let fallbacks = durability.counts().order_fallbacks;
    assert_eq!(barriers, 2 + fallbacks);
}

#[test]
fn init_never_recreates_missing_authoritative_heads_or_receipts() {
    for name in ["heads", "receipts"] {
        let (root, mut comb) = TestComb::new().unwrap().into_parts();
        let actor = agent("fixture", "a", Some("implementor"), "execution", "turn");
        comb.put_markdown(
            &"concepts/io".parse().unwrap(),
            &markdown("Existing"),
            None,
            &fixture_write(&actor, "create", None).unwrap(),
        )
        .unwrap();
        comb.append(
            &"concepts/io".parse().unwrap(),
            "Accepted measurement",
            "test",
            None,
            None,
            &fixture_write(&actor, "append", None).unwrap(),
        )
        .unwrap();
        let page = std::fs::read(root.path().join("concepts/io.md")).unwrap();
        drop(comb);
        let directory = root.path().join(".comb").join(name);
        let retained = root.path().join(".comb").join(format!("retained-{name}"));
        std::fs::rename(&directory, &retained).unwrap();
        assert!(Comb::init(root.path()).is_err());
        assert!(!directory.exists());
        assert_eq!(
            std::fs::read(root.path().join("concepts/io.md")).unwrap(),
            page
        );
        std::fs::rename(retained, directory).unwrap();
        let mut comb = Comb::open(root.path()).unwrap();
        assert_authoritative_receipts_survived(&mut comb, &actor);
    }
}

fn assert_authoritative_receipts_survived(comb: &mut Comb, actor: &Actor) {
    assert!(
        comb.append(
            &"concepts/io".parse().unwrap(),
            "Accepted measurement",
            "test",
            None,
            None,
            &fixture_write(actor, "append", None).unwrap()
        )
        .unwrap()
        .replayed
    );
    assert_eq!(
        comb.get(&"concepts/io".parse().unwrap())
            .unwrap()
            .page
            .timeline
            .len(),
        1
    );
}
