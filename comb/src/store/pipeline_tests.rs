use super::*;
use crate::ErrorCode;
use crate::test_support::{TestComb, write as fixture_write};
use std::fs;

#[test]
fn capture_receipt_hashes_exact_note_input_and_distinguishes_empty_run() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    let text = "\"Quoted\" 日本語\n\n  Body stays raw.  \n";
    let raw = concat!(
        "---\ntitle: \"\\\"Quoted\\\" 日本語\"\ntype: note\n---\n",
        "\"Quoted\" 日本語\n\n  Body stays raw.  \n"
    );
    assert_eq!(Page::note("\"Quoted\" 日本語", text).unwrap(), raw);
    let write = Write::new(&Actor::Human, &"capture".parse().unwrap(), Some("")).unwrap();
    let created = comb.capture(text, &write).unwrap();
    let receipt = comb
        .disk
        .receipt(&write.principal, &write.request_id)
        .unwrap()
        .unwrap();
    let expected = page::hash(
        serde_json::to_vec(&(
            &write.principal,
            ("put", &created.slug, raw, None::<&ContentHash>, Some("")),
        ))
        .unwrap(),
    );
    assert_eq!(receipt.fingerprint, expected);
    assert_eq!(receipt.operation, "put");
    assert!(
        comb.put_markdown(&created.slug, raw, None, &write)
            .unwrap()
            .replayed
    );
    assert_eq!(
        comb.capture(
            text,
            &fixture_write(&Actor::Human, "capture", None).unwrap()
        )
        .unwrap_err()
        .code,
        ErrorCode::RequestConflict
    );
    assert_eq!(
        comb.capture(text.trim_end(), &write).unwrap_err().code,
        ErrorCode::RequestConflict
    );
}

#[test]
fn put_preserves_receipt_parse_cas_and_suffix_error_order() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    let slug = "concepts/order".parse().unwrap();
    let markdown = "---\ntitle: Order\n---\nSummary";
    let appended = seed_appended_page(&mut comb, &slug, markdown);
    let wrong = ContentHash::digest("stale");
    assert_eq!(
        comb.put_markdown(
            &slug,
            "bad",
            Some(&wrong),
            &fixture_write(&Actor::Human, "seed", None).unwrap()
        )
        .unwrap_err()
        .code,
        ErrorCode::RequestConflict
    );
    crate::test_support::assert_input_cause(
        &comb
            .put_markdown(
                &slug,
                "bad",
                Some(&wrong),
                &fixture_write(&Actor::Human, "parse", None).unwrap(),
            )
            .unwrap_err(),
        "/operation/markdown",
        Error::frontmatter_missing(),
    );
    assert_eq!(
        comb.put_markdown(
            &slug,
            markdown,
            Some(&wrong),
            &fixture_write(&Actor::Human, "cas", None).unwrap()
        )
        .unwrap_err()
        .code,
        ErrorCode::Conflict
    );
    assert_eq!(
        comb.put_markdown(
            &slug,
            markdown,
            Some(&appended.hash),
            &fixture_write(&Actor::Human, "suffix", None).unwrap()
        )
        .unwrap_err()
        .message,
        Error::immutable_timeline().message
    );
    assert_eq!(comb.get(&slug).unwrap().hash, appended.hash);
    assert!(!comb.disk.admin.join("intent.json").exists());
}

fn seed_appended_page(comb: &mut Comb, slug: &Slug, markdown: &str) -> Mutation {
    comb.put_markdown(
        slug,
        markdown,
        None,
        &fixture_write(&Actor::Human, "seed", None).unwrap(),
    )
    .unwrap();
    comb.append(
        slug,
        "Evidence",
        "fixture",
        None,
        None,
        &fixture_write(&Actor::Human, "append", None).unwrap(),
    )
    .unwrap()
}

#[test]
fn append_optional_cas_and_pre_replay_evidence_validation_remain_distinct() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    let created = comb
        .capture(
            "Observed fact",
            &fixture_write(&Actor::Human, "capture", None).unwrap(),
        )
        .unwrap();
    let write = fixture_write(&Actor::Human, "append", None).unwrap();
    let appended = comb
        .append(&created.slug, " Fact ", "fixture", None, None, &write)
        .unwrap();
    crate::test_support::assert_input_cause(
        &comb
            .append(&created.slug, " ", "fixture", None, None, &write)
            .unwrap_err(),
        "/operation/text",
        Error::invalid_evidence(),
    );
    assert_eq!(
        comb.append(&created.slug, "Fact", "fixture", None, None, &write)
            .unwrap_err()
            .code,
        ErrorCode::RequestConflict
    );
    let missing_hash = comb
        .append(
            &created.slug,
            "New",
            "fixture",
            Some("Summary"),
            None,
            &fixture_write(&Actor::Human, "compiled", None).unwrap(),
        )
        .unwrap_err();
    assert_eq!(
        missing_hash.input().unwrap().code(),
        hive_input::Code::MissingArgument
    );
    assert_eq!(
        missing_hash.input().unwrap().field(),
        "/operation/expected_hash"
    );
    assert_eq!(
        comb.append(
            &created.slug,
            "New",
            "fixture",
            None,
            Some(&created.hash),
            &fixture_write(&Actor::Human, "stale", None).unwrap()
        )
        .unwrap_err()
        .code,
        ErrorCode::Conflict
    );
    assert_eq!(comb.get(&created.slug).unwrap().hash, appended.hash);
}

#[test]
fn reviewer_checks_and_decision_reason_validation_still_precede_replay() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    let actor = Actor::Client {
        id: "reviewer".into(),
        name: "Reviewer".into(),
    };
    let write = |id: &str| Write::new(&actor, &id.parse().unwrap(), None).unwrap();
    let slug = "review/proposal".parse().unwrap();
    let created = comb
        .put_markdown(
            &slug,
            "---\ntitle: Proposal\n---\nSummary",
            None,
            &fixture_write(&Actor::Human, "seed", None).unwrap(),
        )
        .unwrap();
    let promoted = comb
        .promote(
            &slug,
            &created.hash,
            &write("promote").review_for_promotion(true).unwrap(),
        )
        .unwrap();
    assert_eq!(
        write("promote")
            .review_for_promotion(false)
            .and_then(|reviewed| comb.promote(&slug, &created.hash, &reviewed))
            .unwrap_err()
            .message,
        Error::promotion_reviewer_required().message
    );
    let decided = comb
        .decide(
            &slug,
            &promoted.hash,
            page::Decision::Accept,
            "Supported",
            &write("decide").review_for_decision(true).unwrap(),
        )
        .unwrap();
    assert_decision_replay_precedence(&mut comb, &actor, &slug, &promoted, &decided);
}

fn assert_decision_replay_precedence(
    comb: &mut Comb,
    actor: &Actor,
    slug: &Slug,
    promoted: &Mutation,
    decided: &Mutation,
) {
    let write = |id: &str| Write::new(actor, &id.parse().unwrap(), None).unwrap();
    assert_eq!(
        write("decide")
            .review_for_decision(false)
            .and_then(|reviewed| comb.decide(
                slug,
                &promoted.hash,
                page::Decision::Accept,
                "Supported",
                &reviewed
            ))
            .unwrap_err()
            .message,
        Error::decision_reviewer_required().message
    );
    crate::test_support::assert_input_cause(
        &comb
            .decide(
                slug,
                &promoted.hash,
                page::Decision::Accept,
                " ",
                &write("decide").review_for_decision(true).unwrap(),
            )
            .unwrap_err(),
        "/operation/reason",
        Error::invalid_decision(),
    );
    assert_eq!(comb.get(slug).unwrap().hash, decided.hash);
}

#[test]
fn import_receipts_keep_original_frontmatter_and_raw_note_inputs() {
    let root = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let mut comb = Comb::init(root.path()).unwrap();
    let authored = "---\ntitle: 'Original: 日本語'\n---\n\nBody with trailing spaces.  \n";
    let plain = "# \"Guide\" 日本語\n\nBody with trailing spaces.  \n";
    fs::write(source.path().join("authored.md"), authored).unwrap();
    fs::write(source.path().join("Plain.md"), plain).unwrap();
    comb.import(source.path(), Some(&"operations".parse().unwrap()))
        .unwrap();
    let principal = Principal::for_actor(&Actor::Human).unwrap();
    let mut receipts = fs::read_dir(comb.disk.admin.join("receipts"))
        .unwrap()
        .map(|entry| {
            serde_json::from_slice::<Receipt>(&fs::read(entry.unwrap().path()).unwrap()).unwrap()
        })
        .collect::<Vec<_>>();
    receipts.sort_by(|left, right| left.slug.as_str().cmp(right.slug.as_str()));
    let note = concat!(
        "---\ntitle: \"\\\"Guide\\\" 日本語\"\ntype: note\n---\n",
        "# \"Guide\" 日本語\n\nBody with trailing spaces.  \n"
    );
    for (receipt, raw) in receipts.iter().zip([authored, note]) {
        assert_eq!(
            receipt.fingerprint,
            page::hash(
                serde_json::to_vec(&(
                    &principal,
                    (
                        "put",
                        &receipt.slug,
                        raw,
                        None::<&ContentHash>,
                        None::<&str>
                    )
                ))
                .unwrap()
            )
        );
    }
    assert_eq!(receipts.len(), 2);
    assert_eq!(receipts[0].slug.as_str(), "operations/authored");
    assert_eq!(receipts[1].slug.as_str(), "operations/plain");
}
