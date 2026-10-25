use super::*;
use crate::{
    Actor,
    store::Consolidate,
    test_support::{TestComb, write},
};

fn field<T>(result: Result<T>, expected: &str) -> Error {
    let error = result.err().expect("invalid authored input");
    let input = error.input().expect("typed rejection");
    assert_eq!(input.field(), expected);
    assert!(!input.expected().is_empty());
    assert!(!input.examples().is_empty());
    assert!(!error.file_committed);
    error
}

#[test]
fn authored_text_limits_count_original_utf8_bytes_and_keep_valid_whitespace() {
    for invalid in [" \n\t".into(), "é".repeat(EVIDENCE_TEXT_BYTES / 2 + 1)] {
        field(capture(&invalid), "/operation/text");
        field(evidence(&invalid, "source"), "/operation/text");
    }
    for invalid in [" \n".into(), "x".repeat(SOURCE_BYTES + 1)] {
        field(evidence("text", &invalid), "/operation/source");
    }
    let endpoint = "é".repeat(EVIDENCE_TEXT_BYTES / 2);
    capture(&endpoint).unwrap();
    evidence(&endpoint, &"é".repeat(SOURCE_BYTES / 2)).unwrap();
    evidence(" \ntext\t", " source ").unwrap();
    capture(" \ntext\t").unwrap();
}

#[test]
fn learning_fields_identify_each_native_rule_in_order() {
    let review = "review/proposal".parse().unwrap();
    field(
        decision(&"concepts/page".parse().unwrap(), "reason"),
        "/operation/slug",
    );
    for reason in [" \t".into(), "x".repeat(DECISION_REASON_BYTES + 1)] {
        field(decision(&review, &reason), "/operation/reason");
    }
    decision(&review, &"x".repeat(DECISION_REASON_BYTES)).unwrap();
    let valid = || Consolidate {
        note: "inbox/source".parse().unwrap(),
        target: "concepts/page".parse().unwrap(),
        note_hash: "a".repeat(64).parse().unwrap(),
        target_hash: "b".repeat(64).parse().unwrap(),
        summary: "summary".into(),
        recopy: false,
    };
    consolidation(&valid(), Some("run")).unwrap();
    let mut invalid = valid();
    invalid.note = review.clone();
    field(
        consolidation(&invalid, Some("run")),
        "/operation/request/note",
    );
    for target in ["inbox/destination", "review/proposal"] {
        let mut invalid = valid();
        invalid.target = target.parse().unwrap();
        field(
            consolidation(&invalid, Some("run")),
            "/operation/request/target",
        );
    }
    for run in [None, Some("")] {
        field(consolidation(&valid(), run), "/operation/run");
    }
    let mut invalid = valid();
    invalid.summary = " \n".into();
    field(
        consolidation(&invalid, Some("run")),
        "/operation/request/summary",
    );
}

#[test]
fn put_replay_conflicts_precede_parse_and_capture_keeps_text_attribution() {
    let (_dir, mut store) = TestComb::new().unwrap().into_parts();
    let slug = "concepts/page".parse().unwrap();
    let markdown = "---\ntitle: Example\n---\nText";
    let created = store
        .put_markdown(
            &slug,
            markdown,
            None,
            &write(&Actor::Human, "first", None).unwrap(),
        )
        .unwrap();
    let conflict = store
        .put_markdown(
            &slug,
            "PRIVATE invalid",
            None,
            &write(&Actor::Human, "first", None).unwrap(),
        )
        .unwrap_err();
    assert_eq!(conflict.code, crate::ErrorCode::RequestConflict);
    assert!(conflict.input().is_none());
    let error = field(
        store.put_markdown(
            &slug,
            "PRIVATE invalid",
            None,
            &write(&Actor::Human, "fresh", None).unwrap(),
        ),
        "/operation/markdown",
    );
    assert!(std::error::Error::source(&error).is_some());
    assert!(!serde_json::to_string(&error).unwrap().contains("PRIVATE"));
    for text in [
        "PRIVATE\0text",
        "note\n\n---\n## Timeline\nnot an evidence entry",
    ] {
        field(
            store.capture(text, &write(&Actor::Human, "note", None).unwrap()),
            "/operation/text",
        );
    }
    let stale = "a".repeat(64).parse().unwrap();
    let error = store
        .put_markdown(
            &slug,
            markdown,
            Some(&stale),
            &write(&Actor::Human, "stale", None).unwrap(),
        )
        .unwrap_err();
    assert_eq!(error.code, crate::ErrorCode::Conflict);
    assert!(error.input().is_none());
    assert_eq!(store.get(&slug).unwrap().hash, created.hash);
}

#[test]
fn append_requires_hash_after_replay_and_allows_an_empty_summary() {
    let (_dir, mut store) = TestComb::new().unwrap().into_parts();
    let slug = "concepts/page".parse().unwrap();
    let created = store
        .put_markdown(
            &slug,
            "---\ntitle: Example\n---\nText",
            None,
            &write(&Actor::Human, "create", None).unwrap(),
        )
        .unwrap();
    let invalid = field(
        store.append(
            &slug,
            "text",
            "source",
            Some(""),
            None,
            &write(&Actor::Human, "missing", None).unwrap(),
        ),
        "/operation/expected_hash",
    );
    assert_eq!(invalid.input().unwrap().code(), Code::MissingArgument);
    let write = write(&Actor::Human, "append", None).unwrap();
    let result = store
        .append(
            &slug,
            "text",
            "source",
            Some(""),
            Some(&created.hash),
            &write,
        )
        .unwrap();
    assert!(store.get(&slug).unwrap().page.compiled.is_empty());
    assert!(
        store
            .append(
                &slug,
                "text",
                "source",
                Some(""),
                Some(&created.hash),
                &write
            )
            .unwrap()
            .replayed
    );
    let error = store
        .append(&slug, "text", "source", Some(""), None, &write)
        .unwrap_err();
    assert_eq!(error.code, crate::ErrorCode::RequestConflict);
    assert!(error.input().is_none());
    assert_eq!(store.get(&slug).unwrap().hash, result.hash);
}
