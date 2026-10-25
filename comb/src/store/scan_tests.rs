use super::*;
use crate::ErrorCode;
use std::{fs, os::unix::fs::PermissionsExt};

fn setup() -> (tempfile::TempDir, Comb, Vec<Slug>) {
    let root = tempfile::tempdir().unwrap();
    let mut comb = Comb::init(root.path()).unwrap();
    for name in ["first", "second", "absent"] {
        let slug: Slug = format!("concepts/{name}").parse().unwrap();
        let write = Write::new(&Actor::Human, &name.parse().unwrap(), None).unwrap();
        comb.put_markdown(&slug, "---\ntitle: Original\n---\nOriginal", None, &write)
            .unwrap();
    }
    fs::remove_file(root.path().join("concepts/absent.md")).unwrap();
    let order = fs::read_dir(root.path().join("concepts"))
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            format!("concepts/{}", path.file_stem().unwrap().to_str().unwrap())
                .parse()
                .unwrap()
        })
        .collect::<Vec<Slug>>();
    assert_eq!(order.len(), 2);
    fs::write(
        comb.disk.page_path(&order[0], false).unwrap(),
        "---\ntitle: Changed\n---\nChanged",
    )
    .unwrap();
    (root, comb, order)
}

#[test]
fn iterator_error_after_publication_is_committed_and_keeps_absent_rows() {
    let (_root, mut comb, order) = setup();
    let later = comb.disk.page_path(&order[1], false).unwrap();
    comb.before_index = Some(Box::new(move || {
        fs::set_permissions(later, fs::Permissions::from_mode(0o666)).unwrap();
    }));
    let error = comb.sync().unwrap_err();
    assert_eq!(error.code, ErrorCode::UnsafePath);
    assert!(error.file_committed);
    assert!(error.request_id.is_none());
    assert_eq!(comb.stats().unwrap().pages, 3);
    let published = comb.get(&order[0]).unwrap();
    assert_eq!(published.page.compiled, "Changed");
    assert_eq!(
        comb.index.content_hash(&order[0]).unwrap(),
        Some(published.hash)
    );
    comb.ready().unwrap();
}

#[test]
fn page_parse_error_is_quarantined_and_scan_still_finishes() {
    let (_root, mut comb, order) = setup();
    let later = comb.disk.page_path(&order[1], false).unwrap();
    comb.before_index = Some(Box::new(move || {
        fs::write(later, "Malformed page").unwrap();
    }));
    let stats = comb.sync().unwrap();
    assert_eq!(stats.pages, 1);
    assert_eq!(stats.quarantined_pages, 1);
    assert!(comb.index.content_hash(&order[1]).unwrap().is_none());
    assert_eq!(
        fs::read(comb.disk.page_path(&order[1], false).unwrap()).unwrap(),
        b"Malformed page"
    );
    assert_eq!(comb.get(&order[0]).unwrap().page.compiled, "Changed");
    comb.ready().unwrap();
}

#[test]
fn invalid_utf8_and_oversized_pages_are_retained_reported_and_repairable() {
    let (_root, mut comb, order) = setup();
    let path = comb.disk.page_path(&order[1], false).unwrap();
    for invalid in [vec![0xff], vec![b'x'; crate::page::PAGE_BYTES + 1]] {
        fs::write(&path, &invalid).unwrap();
        let stats = comb.sync().unwrap();
        assert_eq!(stats.quarantined_pages, 1);
        assert!(comb.index.content_hash(&order[1]).unwrap().is_none());
        assert_eq!(fs::read(&path).unwrap(), invalid);
        assert!(
            comb.doctor()
                .unwrap()
                .findings
                .iter()
                .any(|f| f.slug == order[1].as_str() && f.kind == FindingKind::InvalidPage)
        );
    }
    fs::write(&path, "---\ntitle: Repaired\n---\nRepaired").unwrap();
    assert_eq!(comb.sync().unwrap().quarantined_pages, 0);
    assert!(comb.index.content_hash(&order[1]).unwrap().is_some());
    fs::write(&path, "broken").unwrap();
    assert_eq!(comb.sync().unwrap().quarantined_pages, 1);
    fs::remove_file(&path).unwrap();
    assert_eq!(comb.sync().unwrap().quarantined_pages, 0);
}
