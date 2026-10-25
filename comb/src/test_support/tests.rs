use super::*;
use crate::{Actor, Error, ErrorCode};

#[test]
fn write_keeps_explicit_identity_and_run_and_rejects_bad_request_ids() {
    let actor = agent("bridge", "writer", Some("curator"), "execution", "turn");
    let mut fixture = TestComb::new().unwrap();
    let created = fixture
        .comb
        .capture("Body", &write(&actor, "capture", None).unwrap())
        .unwrap();
    for (id, run) in [("none", None), ("empty", Some(""))] {
        let appended = fixture
            .comb
            .append(
                &created.slug,
                "Evidence",
                "fixture",
                None,
                None,
                &write(&actor, id, run).unwrap(),
            )
            .unwrap();
        assert_eq!(appended.request_id.as_str(), id);
        let page = fixture.comb.get(&created.slug).unwrap().page;
        assert_eq!(page.timeline[0].metadata.owned().actor(), &actor);
        assert_eq!(page.timeline[0].metadata.owned().run(), run);
        let principal = crate::values::Principal::for_actor(&actor).unwrap();
        assert!(
            fixture
                .comb
                .disk
                .receipt(&principal, &id.parse().unwrap())
                .unwrap()
                .is_some()
        );
    }
    assert_code!(write(&Actor::Human, "", None), ErrorCode::InvalidRequestId);
}

#[test]
fn page_builder_quotes_authored_text_and_still_uses_strict_validation() {
    let page = PageBuilder::new("A: \"quoted\" 日本語", "Body")
        .kind("component")
        .tags(&["rust"])
        .aliases(&["Alias"])
        .build()
        .unwrap();
    assert_eq!(page.metadata.authored.title, "A: \"quoted\" 日本語");
    assert_eq!(page.metadata.authored.kind, "component");
    assert_eq!(page.metadata.authored.tags, ["rust"]);
    assert_eq!(page.metadata.authored.aliases, ["Alias"]);
    assert_code!(PageBuilder::new("", "Body").build(), ErrorCode::InvalidPage);
}

#[test]
fn code_assertion_evaluates_once_and_returns_full_error() {
    let mut calls = 0;
    let mut expected = Error::request_conflict()
        .caused_by(std::io::Error::from_raw_os_error(libc::EIO))
        .committed();
    expected.request_id = Some("fixture".into());
    let error = assert_code!(
        {
            calls += 1;
            Err::<(), _>(expected.clone())
        },
        ErrorCode::RequestConflict
    );
    assert_eq!(calls, 1);
    assert_eq!(error.message, expected.message);
    assert_eq!(error.request_id, expected.request_id);
    assert!(error.file_committed);
    assert_eq!(
        crate::error::tests::io_cause(&error).raw_os_error(),
        Some(libc::EIO)
    );
}

#[test]
fn short_comb_can_transfer_ownership_for_an_immediate_reopen() {
    let fixture = TestComb::short().unwrap();
    let (root, comb) = fixture.into_parts();
    comb.ready().unwrap();
    drop(comb);
    Comb::open(root.path()).unwrap().ready().unwrap();
}
