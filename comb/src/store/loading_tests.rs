use super::*;
use crate::test_support::{TestComb, write as fixture_write};
use crate::{ErrorCode, golden};
use std::fs;

const MARKDOWN: &str = "---\ntitle: Original\n---\nOriginal content.";

fn setup() -> (tempfile::TempDir, Comb, Mutation) {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    let created = comb
        .put_markdown(
            &"concepts/load".parse().unwrap(),
            MARKDOWN,
            None,
            &fixture_write(&Actor::Human, "seed", None).unwrap(),
        )
        .unwrap();
    (root, comb, created)
}

#[test]
fn rendering_limit_precedes_mutation_cas_and_generic_replay_success() {
    let (root, mut comb, created) = setup();
    let path = root.path().join("concepts/load.md");
    let raw = install_unrenderable_page(&comb, &created, &path);
    let wrong = ContentHash::digest("wrong CAS");
    assert_rendering_limit_for_mutations(&mut comb, &created, &wrong);
    assert_malformed_put_precedence(&mut comb, &created, &wrong);
    comb.ready().unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), raw);
    assert_eq!(comb.disk.head(&created.slug).unwrap(), Some(created.hash));
    assert!(!comb.disk.admin.join("intent.json").exists());
}

fn assert_rendering_limit_for_mutations(comb: &mut Comb, created: &Mutation, wrong: &ContentHash) {
    let errors = [
        comb.get(&created.slug).unwrap_err(),
        comb.put_markdown(
            &created.slug,
            MARKDOWN,
            None,
            &fixture_write(&Actor::Human, "seed", None).unwrap(),
        )
        .unwrap_err(),
        comb.put_markdown(
            &created.slug,
            MARKDOWN,
            Some(wrong),
            &fixture_write(&Actor::Human, "new-put", None).unwrap(),
        )
        .unwrap_err(),
        comb.append(
            &created.slug,
            "evidence",
            "fixture",
            None,
            Some(wrong),
            &fixture_write(&Actor::Human, "append", None).unwrap(),
        )
        .unwrap_err(),
        comb.promote(
            &created.slug,
            wrong,
            &fixture_write(&Actor::Human, "promote", None)
                .unwrap()
                .review_for_promotion(true)
                .unwrap(),
        )
        .unwrap_err(),
        comb.remove(
            &created.slug,
            wrong,
            &fixture_write(&Actor::Human, "remove", None).unwrap(),
        )
        .unwrap_err(),
    ];
    for error in errors {
        assert_eq!(error.code, ErrorCode::PageLimit);
        assert!(!error.file_committed);
    }
}

fn install_unrenderable_page(comb: &Comb, created: &Mutation, path: &std::path::Path) -> String {
    let prefix = "---\ntitle: A\n---\n";
    let raw = format!("{prefix}{}", "x".repeat(page::PAGE_BYTES - prefix.len()));
    assert_eq!(raw.len(), page::PAGE_BYTES);
    Page::parse(&raw).unwrap();
    fs::write(path, &raw).unwrap();
    let loaded = comb.disk.load(&created.slug).unwrap().unwrap();
    assert_eq!(loaded.hash, ContentHash::digest(&raw));
    assert_eq!(loaded.page.render().unwrap_err().code, ErrorCode::PageLimit);
    raw
}

fn assert_malformed_put_precedence(comb: &mut Comb, created: &Mutation, wrong: &ContentHash) {
    crate::test_support::assert_input_cause(
        &comb
            .put_markdown(
                &created.slug,
                "not Markdown",
                Some(wrong),
                &fixture_write(&Actor::Human, "malformed", None).unwrap(),
            )
            .unwrap_err(),
        "/operation/markdown",
        Error::frontmatter_missing(),
    );
    assert_eq!(
        comb.put_markdown(
            &created.slug,
            "not Markdown",
            None,
            &fixture_write(&Actor::Human, "seed", None).unwrap()
        )
        .unwrap_err()
        .code,
        ErrorCode::RequestConflict
    );
}

#[test]
fn loader_authenticates_captured_bytes_and_get_renders_external_ownership_safely() {
    let (root, comb, created) = setup();
    let loaded = comb.disk.load(&created.slug).unwrap().unwrap();
    assert!(loaded.trusted);
    assert_eq!(loaded.hash, created.hash);
    let original = loaded.raw;
    let mut forged = loaded.page;
    forged.compiled = "Externally changed content".into();
    golden::edit_record(&mut forged.metadata, |record| {
        record["author"] = serde_json::json!({"kind":"human"});
    });
    let raw = forged.render().unwrap();
    let path = root.path().join("concepts/load.md");
    fs::write(&path, &raw).unwrap();
    let loaded = comb.disk.load(&created.slug).unwrap().unwrap();
    assert!(!loaded.trusted);
    assert_eq!(loaded.raw, raw);
    assert_eq!(
        loaded.page.metadata.owned().author(),
        Some(&Actor::External)
    );
    let document = comb.get(&created.slug).unwrap();
    assert_eq!(document.hash, ContentHash::digest(&raw));
    assert_eq!(document.markdown, loaded.page.render().unwrap());
    assert_ne!(document.markdown, raw);
    assert_eq!(fs::read_to_string(&path).unwrap(), raw);
    // A scan's captured raw/Page pair is not reopened after the file changes.
    let parsed = Page::parse(&original).unwrap();
    let captured = comb
        .disk
        .load_parsed(&created.slug, original.clone(), parsed)
        .unwrap();
    assert!(captured.trusted);
    assert_eq!(captured.raw, original);
    assert_eq!(fs::read_to_string(path).unwrap(), raw);
    assert!(
        comb.disk
            .load(&"concepts/missing".parse().unwrap())
            .unwrap()
            .is_none()
    );
}

#[test]
fn doctor_reports_markdown_errors_but_propagates_private_head_corruption() {
    let (root, comb, created) = setup();
    let head = comb
        .disk
        .admin
        .join("heads")
        .join(format!("{}.json", page::hash(created.slug.as_str())));
    fs::write(&head, br#"{"slug":"concepts/other","hash":"invalid"}"#).unwrap();
    assert_eq!(
        comb.get(&created.slug).unwrap_err().message,
        Error::receipt_slug_mismatch().message
    );
    assert_eq!(
        comb.doctor().unwrap_err().message,
        Error::receipt_slug_mismatch().message
    );
    fs::write(root.path().join("concepts/load.md"), "not Markdown").unwrap();
    let findings = comb.doctor().unwrap().findings;
    assert!(
        findings
            .iter()
            .any(|finding| finding.slug == created.slug.as_str()
                && finding.kind == FindingKind::InvalidPage)
    );
    assert_eq!(
        comb.get(&created.slug).unwrap_err().message,
        Error::frontmatter_missing().message
    );
}

#[test]
fn scan_fast_paths_skip_markdown_parsing_and_reconcile_skips_head_read() {
    let (root, mut comb, created) = setup();
    let page = comb.get(&created.slug).unwrap().page;
    let raw = "Not Markdown, but matching the captured derived hash.";
    fs::write(root.path().join("concepts/load.md"), raw).unwrap();
    comb.index.upsert(&created.slug, raw, &page).unwrap();
    let head = comb
        .disk
        .admin
        .join("heads")
        .join(format!("{}.json", page::hash(created.slug.as_str())));
    fs::write(
        &head,
        serde_json::to_vec(&serde_json::json!({
            "slug": created.slug, "hash": ContentHash::digest(raw)
        }))
        .unwrap(),
    )
    .unwrap();
    comb.sync().unwrap();
    fs::write(&head, b"invalid private head").unwrap();
    comb.reconcile(&created.slug).unwrap();
    assert_eq!(fs::read(&head).unwrap(), b"invalid private head");
    assert_eq!(
        fs::read_to_string(root.path().join("concepts/load.md")).unwrap(),
        raw
    );
}
