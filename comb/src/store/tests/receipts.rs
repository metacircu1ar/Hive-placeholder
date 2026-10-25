use super::*;

#[test]
fn sealed_pages_reopen_and_request_replay_survives_index_rebuild() {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    let actor = agent("fixture", "a", Some("implementor"), "execution", "turn");
    comb.put_markdown(
        &"components/writer".parse().unwrap(),
        &markdown("Durable writer"),
        None,
        &fixture_write(&actor, "create", None).unwrap(),
    )
    .unwrap();
    comb.append(
        &"components/writer".parse().unwrap(),
        "One measurement",
        "commit:abc",
        None,
        None,
        &fixture_write(&actor, "append", None).unwrap(),
    )
    .unwrap();
    drop(comb);
    let mut comb = Comb::rebuild(root.path()).unwrap();
    let reply = comb
        .append(
            &"components/writer".parse().unwrap(),
            "One measurement",
            "commit:abc",
            None,
            None,
            &fixture_write(&actor, "append", None).unwrap(),
        )
        .unwrap();
    assert!(reply.replayed);
    assert_eq!(
        comb.get(&"components/writer".parse().unwrap())
            .unwrap()
            .page
            .timeline
            .len(),
        1
    );
    assert_eq!(
        comb.append(
            &"components/writer".parse().unwrap(),
            "Different",
            "commit:abc",
            None,
            None,
            &fixture_write(&actor, "append", None).unwrap()
        )
        .unwrap_err()
        .code,
        crate::ErrorCode::RequestConflict
    );
}

#[test]
fn request_ids_are_scoped_to_principals_and_survive_removed_pages() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    comb.put_markdown(
        &"concepts/shared".parse().unwrap(),
        &markdown("Shared"),
        None,
        &fixture_write(&Actor::Human, "create", None).unwrap(),
    )
    .unwrap();
    let mut captures = BTreeSet::new();
    for actor in [
        agent("fixture", "a", Some("implementor"), "execution", "turn"),
        agent("fixture", "b", Some("implementor"), "execution", "turn"),
        Actor::Human,
    ] {
        comb.append(
            &"concepts/shared".parse().unwrap(),
            "Independent measurement",
            "test",
            None,
            None,
            &fixture_write(&actor, "r1", None).unwrap(),
        )
        .unwrap();
        assert!(
            comb.append(
                &"concepts/shared".parse().unwrap(),
                "Independent measurement",
                "test",
                None,
                None,
                &fixture_write(&actor, "r1", None).unwrap()
            )
            .unwrap()
            .replayed
        );
        assert_capture_receipt_survives_removal(&mut comb, &actor, &mut captures);
    }
    assert_eq!(
        comb.get(&"concepts/shared".parse().unwrap())
            .unwrap()
            .page
            .timeline
            .len(),
        3
    );
}

fn assert_capture_receipt_survives_removal(
    comb: &mut Comb,
    actor: &Actor,
    captures: &mut BTreeSet<Slug>,
) {
    let note = comb
        .capture(
            "Independent note",
            &fixture_write(actor, "r2", None).unwrap(),
        )
        .unwrap();
    assert!(captures.insert(note.slug.clone()));
    assert!(
        comb.capture(
            "Independent note",
            &fixture_write(actor, "r2", None).unwrap()
        )
        .unwrap()
        .replayed
    );
    comb.remove(
        &note.slug,
        &note.hash,
        &fixture_write(&Actor::Human, &format!("remove-{}", note.slug), None).unwrap(),
    )
    .unwrap();
    assert_eq!(
        comb.capture(
            "Independent note",
            &fixture_write(actor, "r2", None).unwrap()
        )
        .unwrap_err()
        .code,
        crate::ErrorCode::ResultRemoved
    );
}

#[test]
fn many_writes_keep_frontmatter_bounded_and_old_receipts_replay() {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    let actor = agent("fixture", "a", Some("implementor"), "execution", "turn");
    let first = markdown("First");
    let mut current = comb
        .put_markdown(
            &"concepts/busy".parse().unwrap(),
            &first,
            None,
            &fixture_write(&actor, "first", None).unwrap(),
        )
        .unwrap();
    for i in 0..1000 {
        current = comb
            .put_markdown(
                &"concepts/busy".parse().unwrap(),
                &markdown(&format!("Summary {i}")),
                Some(&current.hash),
                &fixture_write(&actor, &format!("write-{i}"), None).unwrap(),
            )
            .unwrap();
    }
    assert!(
        comb.get(&"concepts/busy".parse().unwrap())
            .unwrap()
            .markdown
            .len()
            < 4096
    );
    drop(comb);
    let mut comb = Comb::rebuild(root.path()).unwrap();
    assert!(
        comb.put_markdown(
            &"concepts/busy".parse().unwrap(),
            &first,
            None,
            &fixture_write(&actor, "first", None).unwrap()
        )
        .unwrap()
        .replayed
    );
    assert_eq!(
        comb.get(&"concepts/busy".parse().unwrap())
            .unwrap()
            .page
            .compiled,
        "Summary 999"
    );
}

#[test]
fn large_authored_page_is_reported_near_limit() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    let actor = agent("fixture", "a", Some("implementor"), "execution", "turn");
    let big = markdown(&"x".repeat(page::PAGE_BYTES * 4 / 5));
    comb.put_markdown(
        &"concepts/large".parse().unwrap(),
        &big,
        None,
        &fixture_write(&actor, "large", None).unwrap(),
    )
    .unwrap();
    assert!(
        comb.doctor()
            .unwrap()
            .findings
            .iter()
            .any(|f| f.slug == "concepts/large" && f.kind == FindingKind::PageNearLimit)
    );
}

#[test]
fn oversized_principal_is_rejected_before_persisting_recovery_state() {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    let actor = agent(
        "fixture",
        &"x".repeat(20_000),
        Some("implementor"),
        "execution",
        "turn",
    );
    assert_eq!(
        Write::new(&actor, &"create".parse().unwrap(), None)
            .err()
            .unwrap()
            .code,
        crate::ErrorCode::InvalidActor
    );
    assert!(!root.path().join(".comb/intent.json").exists());
    assert!(!root.path().join("concepts/oversized.md").exists());
    comb.put_markdown(
        &"concepts/valid".parse().unwrap(),
        &markdown("Valid"),
        None,
        &fixture_write(&Actor::Human, "create", None).unwrap(),
    )
    .unwrap();
}

#[test]
fn malformed_receipt_fingerprint_remains_request_conflict() {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    let actor = Actor::Human;
    let write = fixture_write(&actor, "fingerprint", None).unwrap();
    let slug = "concepts/fingerprint".parse().unwrap();
    let markdown = markdown("Original");
    comb.put_markdown(&slug, &markdown, None, &write).unwrap();
    let path = std::fs::read_dir(root.path().join(".comb/receipts"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    receipt["fingerprint"] = serde_json::json!("malformed");
    std::fs::write(&path, serde_json::to_vec(&receipt).unwrap()).unwrap();
    assert_eq!(
        comb.put_markdown(&slug, &markdown, None, &write)
            .unwrap_err()
            .code,
        crate::ErrorCode::RequestConflict
    );
}

#[test]
fn replay_does_not_interpret_stored_operation_claims() {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    let actor = Actor::Human;
    let write = fixture_write(&actor, "opaque-operation", None).unwrap();
    let slug = "concepts/opaque".parse().unwrap();
    let markdown = markdown("Original");
    let created = comb.put_markdown(&slug, &markdown, None, &write).unwrap();
    let path = std::fs::read_dir(root.path().join(".comb/receipts"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for operation in ["", "arbitrary opaque claim"] {
        receipt["operation"] = serde_json::json!(operation);
        let bytes = serde_json::to_vec(&receipt).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let replay = comb.put_markdown(&slug, &markdown, None, &write).unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.hash, created.hash);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}
