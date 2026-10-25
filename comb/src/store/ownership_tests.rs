use super::*;
use crate::test_support::{TestComb, client, write as fixture_write};
use crate::{ErrorCode, disk::golden::seeded_comb, golden};
use serde_json::json;

#[test]
fn put_replaces_new_owned_claims_but_fingerprints_original_markdown() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    let slug = "concepts/claims".parse().unwrap();
    let actor = client("writer", "Writer");
    let mut submitted = golden::page(Actor::Human);
    golden::edit_record(&mut submitted.metadata, |raw| {
        raw["external"] = json!(true);
        raw["seal"] = json!("forged seal");
    });
    let markdown = submitted.render().unwrap();
    let created = comb
        .put_markdown(
            &slug,
            &markdown,
            None,
            &fixture_write(&actor, "claims", Some("current-run")).unwrap(),
        )
        .unwrap();
    let stored = comb.get(&slug).unwrap().page;
    assert_replaced_claims(&stored, &submitted, &actor);
    let replay = comb
        .put_markdown(
            &slug,
            &markdown,
            None,
            &fixture_write(&actor, "claims", Some("current-run")).unwrap(),
        )
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.hash, created.hash);
    assert_eq!(
        comb.put_markdown(
            &slug,
            &format!("{markdown}\n"),
            None,
            &fixture_write(&actor, "claims", Some("current-run")).unwrap()
        )
        .unwrap_err()
        .code,
        ErrorCode::RequestConflict
    );
}

fn assert_replaced_claims(stored: &Page, submitted: &Page, actor: &Actor) {
    let owned = stored.metadata.owned();
    assert_eq!(owned.author(), Some(actor));
    assert!(!owned.external());
    assert!(owned.promotion().is_none());
    assert!(owned.consolidated_into().is_empty());
    assert!(owned.decision().is_none());
    assert_ne!(owned.created_at().unwrap().as_str(), golden::AT);
    assert_ne!(owned.updated_at().unwrap().as_str(), golden::AT);
    assert_ne!(owned.seal(), Some("forged seal"));
    assert!(!stored.verified());
    assert_eq!(
        stored.compiled_digest().unwrap(),
        submitted.compiled_digest().unwrap()
    );
    for (saved, claim) in stored.timeline.iter().zip(&submitted.timeline) {
        assert_eq!(saved.metadata.authored, claim.metadata.authored);
        assert_eq!(saved.text, claim.text);
        assert_eq!(saved.metadata.owned().actor(), actor);
        assert_eq!(saved.metadata.owned().run(), Some("current-run"));
        assert_ne!(saved.metadata.owned().at().as_str(), golden::AT);
        assert!(saved.metadata.owned().consolidation().is_none());
    }
    assert_eq!(stored.timeline.len(), submitted.timeline.len());
}

#[test]
fn put_compares_every_retained_owned_field_before_discarding_claims() {
    let (_root, mut comb) = seeded_comb(&[("concepts/history", golden::page(Actor::Human))]);
    let slug = "concepts/history".parse().unwrap();
    let prior = comb.get(&slug).unwrap();
    let actor = client("writer", "Writer");
    for (field, value) in [
        ("at", json!("2027-01-02T03:04:05Z")),
        ("actor", json!(Actor::External)),
        ("run", json!("forged-run")),
        ("consolidation", json!(null)),
    ] {
        let mut edited = prior.page.clone();
        golden::edit_record(&mut edited.timeline[0].metadata, |raw| raw[field] = value);
        let error = comb
            .put_markdown(
                &slug,
                &edited.render().unwrap(),
                Some(&prior.hash),
                &fixture_write(&actor, field, Some("current-run")).unwrap(),
            )
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::ImmutableTimeline, "{field}");
        assert!(!error.file_committed);
        assert_eq!(comb.get(&slug).unwrap().hash, prior.hash);
        assert!(
            comb.disk
                .receipt(&actor.principal().unwrap(), &field.parse().unwrap())
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn put_keeps_loaded_creation_promotion_and_checkpoints_and_reopens_decision() {
    let mut seed = golden::page(Actor::Human);
    golden::edit_record(&mut seed.metadata, |raw| raw["created_at"] = json!(""));
    let (_root, mut comb) = seeded_comb(&[("concepts/retention", seed)]);
    let slug = "concepts/retention".parse().unwrap();
    let prior = comb.get(&slug).unwrap();
    let mut submitted = prior.page.clone();
    submitted.compiled = "Changed interpretation".into();
    golden::edit_record(&mut submitted.metadata, |raw| {
        raw["created_at"] = json!(golden::AT);
        raw["promotion"] = json!(null);
        raw["consolidated_into"] = json!([]);
        raw["author"] = json!(Actor::Human);
    });
    let actor = client("writer", "Writer");
    comb.put_markdown(
        &slug,
        &submitted.render().unwrap(),
        Some(&prior.hash),
        &fixture_write(&actor, "edit", Some("current-run")).unwrap(),
    )
    .unwrap();
    let stored = comb.get(&slug).unwrap().page;
    assert!(stored.metadata.owned().created_at().is_none());
    assert_eq!(
        crate::error::value(&stored.metadata).unwrap()["created_at"],
        ""
    );
    assert_eq!(
        stored.metadata.owned().promotion(),
        prior.page.metadata.owned().promotion()
    );
    assert_eq!(
        stored.metadata.owned().consolidated_into(),
        prior.page.metadata.owned().consolidated_into()
    );
    assert!(stored.metadata.owned().decision().is_none());
    assert_eq!(stored.metadata.owned().author(), Some(&actor));
    assert!(!stored.metadata.owned().external());
    assert_eq!(stored.timeline, prior.page.timeline);
    assert!(!stored.verified());
}

#[test]
fn ignored_put_ownership_is_still_parsed_before_cas() {
    let (_root, mut comb) = seeded_comb(&[("concepts/history", golden::page(Actor::Human))]);
    let slug = "concepts/history".parse().unwrap();
    let before = comb.get(&slug).unwrap();
    let actor = client("writer", "Writer");
    let bad_date = "---\ntitle: Claimed\ncreated_at: not-a-date\n---\nText";
    let error = comb
        .put_markdown(
            &slug,
            bad_date,
            None,
            &fixture_write(&actor, "bad-date", Some("current-run")).unwrap(),
        )
        .unwrap_err();
    crate::test_support::assert_input_cause(
        &error,
        "/operation/markdown",
        Error::invalid_page_timestamps(),
    );
    let bad_entry = format!(
        "---\ntitle: Claimed\n---\nText\n\n---\n## Timeline\n\n<!-- comb-entry {} -->\nEvidence\n",
        json!({"id":golden::ID,"at":"not-a-date","source":"test","actor":{"kind":"human"},"run":null})
    );
    let error = comb
        .put_markdown(
            &slug,
            &bad_entry,
            None,
            &fixture_write(&actor, "bad-entry", Some("current-run")).unwrap(),
        )
        .unwrap_err();
    crate::test_support::assert_input_cause(
        &error,
        "/operation/markdown",
        Error::invalid_evidence_timestamp(),
    );
    assert_eq!(comb.get(&slug).unwrap().hash, before.hash);
}

fn external_page() -> (tempfile::TempDir, Comb, Slug) {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    std::fs::create_dir(root.path().join("concepts")).unwrap();
    std::fs::write(
        root.path().join("concepts/external.md"),
        golden::page(Actor::Human).render().unwrap(),
    )
    .unwrap();
    comb.sync().unwrap();
    (root, comb, "concepts/external".parse().unwrap())
}

#[test]
fn append_preserves_external_summary_until_a_human_edits_it() {
    for actor in [client("writer", "Writer"), Actor::Human] {
        for summary in [None, Some("Revised summary [evidence:new]")] {
            let (_root, mut comb, slug) = external_page();
            let prior = comb.get(&slug).unwrap();
            comb.append(
                &slug,
                "Fresh evidence",
                "test",
                summary,
                summary.map(|_| &prior.hash),
                &fixture_write(&actor, "append", Some("current-run")).unwrap(),
            )
            .unwrap();
            let stored = comb.get(&slug).unwrap().page;
            assert_eq!(
                stored.metadata.owned().external(),
                !(actor.is_human() && summary.is_some())
            );
            assert_eq!(
                stored.metadata.owned().author(),
                Some(if summary.is_some() {
                    &actor
                } else {
                    &Actor::External
                })
            );
            assert_eq!(stored.verified(), actor.is_human() && summary.is_some());
            assert_eq!(stored.timeline[0].metadata.owned().actor(), &actor);
            assert_eq!(&stored.timeline[1..], &prior.page.timeline);
            assert!(
                stored.timeline[1..]
                    .iter()
                    .all(|entry| entry.metadata.owned().actor() == &Actor::External)
            );
            assert!(stored.metadata.owned().decision().is_none());
        }
    }
}

#[test]
fn put_uses_the_loaded_external_suffix_without_restoring_authorship() {
    let (_root, mut comb, slug) = external_page();
    let prior = comb.get(&slug).unwrap();
    let mut submitted = prior.page.clone();
    submitted
        .timeline
        .insert(0, golden::page(Actor::Human).timeline.remove(0));
    let actor = client("writer", "Writer");
    let duplicate = comb
        .put_markdown(
            &slug,
            &submitted.render().unwrap(),
            Some(&prior.hash),
            &fixture_write(&actor, "put", Some("current-run")).unwrap(),
        )
        .unwrap_err();
    crate::test_support::assert_input_cause(
        &duplicate,
        "/operation/markdown",
        Error::duplicate_evidence_id(),
    );
    // A new entry still needs a distinct textual ID before ownership is replaced.
    submitted.timeline[0].metadata.authored.id = EvidenceId::generate();
    comb.put_markdown(
        &slug,
        &submitted.render().unwrap(),
        Some(&prior.hash),
        &fixture_write(&actor, "put", Some("current-run")).unwrap(),
    )
    .unwrap();
    let stored = comb.get(&slug).unwrap().page;
    assert!(!stored.metadata.owned().external());
    assert_eq!(stored.metadata.owned().author(), Some(&actor));
    assert!(stored.metadata.owned().promotion().is_none());
    assert_eq!(stored.timeline[0].metadata.owned().actor(), &actor);
    assert!(
        stored.timeline[0]
            .metadata
            .owned()
            .consolidation()
            .is_none()
    );
    assert_eq!(&stored.timeline[1..], &prior.page.timeline);
}

#[test]
fn consolidation_preserves_external_until_human_summary_promotion() {
    for actor in [client("writer", "Writer"), Actor::Human] {
        let (_root, mut comb, slug) = external_page();
        let prior = comb.get(&slug).unwrap();
        let note = comb
            .capture(
                "New observation",
                &fixture_write(&actor, "capture", Some("current-run")).unwrap(),
            )
            .unwrap();
        let request = Consolidate {
            note: note.slug,
            target: slug.clone(),
            note_hash: note.hash,
            target_hash: prior.hash,
            summary: "Consolidated [evidence:new]".into(),
            recopy: false,
        };
        comb.consolidate(
            &request,
            &fixture_write(&actor, "consolidate", Some("current-run")).unwrap(),
        )
        .unwrap();
        let stored = comb.get(&slug).unwrap().page;
        assert_eq!(stored.metadata.owned().external(), !actor.is_human());
        assert_eq!(stored.metadata.owned().author(), Some(&actor));
        assert_eq!(stored.verified(), actor.is_human());
        assert_eq!(&stored.timeline[1..], &prior.page.timeline);
        assert!(
            stored.timeline[0]
                .metadata
                .owned()
                .consolidation()
                .is_some()
        );
        assert_eq!(stored.timeline[0].metadata.owned().actor(), &actor);
    }
}
