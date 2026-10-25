use super::*;
use crate::{Actor, golden};
use rusqlite::types::{FromSql, ToSql, ToSqlOutput, ValueRef};

#[test]
fn domain_bindings_match_real_index_rows_and_reject_unknown_values() {
    let mut page = golden::page(Actor::Human);
    page.compiled = "[[concepts/target]] and [target](target.md)".into();
    let slug = "concepts/source".parse().unwrap();
    assert_eq!(
        crate::chunk::links(&slug, &page)
            .into_iter()
            .map(|link| (link.target, link.kind))
            .collect::<Vec<_>>(),
        vec![
            ("concepts/target".parse().unwrap(), LinkKind::Markdown),
            ("concepts/target".parse().unwrap(), LinkKind::Slug),
        ]
    );
    let (_root, comb) = crate::disk::golden::seeded_comb(&[(slug.as_str(), page)]);
    let db = comb.index.test_connection();
    let zones: Vec<(String, Zone)> = db
        .prepare("SELECT DISTINCT zone,zone FROM chunks ORDER BY zone")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        zones,
        vec![
            ("compiled".into(), Zone::Compiled),
            ("timeline".into(), Zone::Timeline),
        ]
    );
    let links: Vec<(String, LinkKind)> = db
        .prepare("SELECT kind,kind FROM links ORDER BY kind")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        links,
        vec![
            ("markdown".into(), LinkKind::Markdown),
            ("slug".into(), LinkKind::Slug),
        ]
    );
    assert!(matches!(
        Zone::Compiled.to_sql().unwrap(),
        ToSqlOutput::Borrowed(ValueRef::Text(b"compiled"))
    ));
    assert!(matches!(
        LinkKind::Slug.to_sql().unwrap(),
        ToSqlOutput::Borrowed(ValueRef::Text(b"slug"))
    ));
    assert_eq!(serde_json::to_string(&LinkKind::Slug).unwrap(), "\"slug\"");
    assert_eq!(
        serde_json::from_str::<LinkKind>("\"slug\"").unwrap(),
        LinkKind::Slug
    );
    assert!(serde_json::from_str::<LinkKind>("\"unknown\"").is_err());
    let error = db
        .execute("UPDATE links SET kind='unknown' WHERE kind='slug'", [])
        .unwrap_err();
    assert_eq!(
        error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::ConstraintViolation)
    );
    assert!(Zone::column_result(ValueRef::Text(b"unknown")).is_err());
    assert!(LinkKind::column_result(ValueRef::Text(b"Slug")).is_err());
    assert!(serde_json::from_str::<Route>("\"hybrid\"").is_err());
    assert!(serde_json::from_str::<FindingKind>("\"new_finding\"").is_err());
}

#[test]
fn filing_prefix_recognition_is_distinct_from_descendant_policy() {
    let published = [
        "components",
        "decisions",
        "incidents",
        "concepts",
        "people",
        "operations",
        "inbox",
        "review",
        "reports",
    ];
    assert_eq!(
        Prefix::ALL
            .iter()
            .map(|prefix| prefix.as_str())
            .collect::<Vec<_>>(),
        published
    );
    for prefix in Prefix::ALL {
        let bare: Slug = prefix.as_str().parse().unwrap();
        let child: Slug = format!("{}/child", prefix.as_str()).parse().unwrap();
        let sibling: Slug = format!("{}ish/child", prefix.as_str()).parse().unwrap();
        assert_eq!(Prefix::recognize(&bare), Some(*prefix));
        assert_eq!(Prefix::recognize(&child), Some(*prefix));
        assert!(!prefix.contains_descendant(&bare));
        assert!(prefix.contains_descendant(&child));
        assert!(!prefix.contains_descendant(&sibling));
        assert_eq!(Prefix::recognize(&sibling), None);
        assert!(!prefix.description().is_empty());
    }
    for slug in ["inbox", "review", "inboxish/child", "custom/child"] {
        let slug = slug.parse().unwrap();
        assert!(!Prefix::is_inbox(&slug));
        assert!(!Prefix::is_review(&slug));
    }
    let captured = Prefix::capture_slug("receipt-hash").unwrap();
    assert_eq!(captured.as_str(), "inbox/receipt-hash");
    assert!(Prefix::is_inbox(&captured));
    assert!(Prefix::is_review(&"review/proposal".parse().unwrap()));
    assert!(Prefix::capture_slug("../outside").is_err());
}
