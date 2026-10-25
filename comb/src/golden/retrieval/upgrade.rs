//! A generated, private corpus retains its real seals and ownership identities.
use super::*;
use crate::{
    service::auth::{Connection, ConnectionExt, Grant, Registry},
    store::{Consolidate, Write},
};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

fn image(root: &Path) -> BTreeMap<PathBuf, (u64, u32, Vec<u8>)> {
    let mut image = BTreeMap::new();
    let mut pending = vec![root.to_owned()];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let meta = entry.path().symlink_metadata().unwrap();
            if meta.is_dir() {
                pending.push(entry.path());
            } else {
                assert!(meta.is_file());
                image.insert(
                    entry.path().strip_prefix(root).unwrap().into(),
                    (meta.ino(), meta.mode(), fs::read(entry.path()).unwrap()),
                );
            }
        }
    }
    image
}

fn searches(comb: &Comb) -> Vec<serde_json::Value> {
    let mut model = Deterministic::new();
    let mut results = Vec::new();
    for query in ["writer", "reviewedproof", "newproof", "approval"] {
        for mode in [Mode::Text, Mode::Vector, Mode::Hybrid] {
            let request = Search {
                query: query.into(),
                ..Search::default()
            };
            let lease = comb
                .index()
                .unwrap()
                .embedding_lease(model.space())
                .unwrap();
            let vector = embedding::run_query(&mut model, lease, query).unwrap();
            let result = comb
                .index()
                .unwrap()
                .retrieve(&request, mode, QueryVector::Vector(vector))
                .unwrap();
            assert!(!result.results.is_empty());
            results.push(json!({"result":result,
                 "score_bits":result.results.iter().map(|r| r.score.to_bits()).collect::<Vec<_>>()}));
        }
    }
    results
}

#[test]
fn synthetic_release_corpus_preserves_trust_receipts_credentials_and_search() {
    let (root, mut comb) = seeded_comb(&corpus());
    let actor = Actor::Client {
        id: golden::ID.into(),
        name: "Corpus writer".into(),
    };
    let (consolidate, proposal_hash, captured_slug) = seed_mutations(&mut comb, &actor);
    let proposal = "review/proposal".parse().unwrap();
    let (connection_path, principal) = seed_credential(&comb, root.path());
    fill(&mut comb, &mut Deterministic::new());
    let slugs = [
        "concepts/writer",
        "concepts/promoted",
        "inbox/note",
        "review/proposal",
    ];
    let documents: Vec<_> = slugs
        .iter()
        .map(|slug| json!(comb.get(&slug.parse().unwrap()).unwrap()))
        .collect();
    assert!(
        !comb
            .get(&"concepts/promoted".parse().unwrap())
            .unwrap()
            .unverified
    );
    assert!(
        comb.get(&proposal)
            .unwrap()
            .page
            .metadata
            .owned()
            .decision()
            .is_some()
    );
    let before_search = searches(&comb);
    drop(comb);
    let before = image(root.path());
    for dry_run in [true, false] {
        let report = Comb::upgrade(root.path(), dry_run).unwrap();
        assert_eq!(report.status, "current");
        assert!(report.steps.is_empty() && report.backups.is_empty());
        assert_eq!(image(root.path()), before);
    }
    let mut comb = Comb::open(root.path()).unwrap();
    assert_eq!(searches(&comb), before_search);
    let registry = Registry::open(&comb.disk, comb.health.clone()).unwrap();
    let restored = Connection::read(&connection_path).unwrap();
    assert_eq!(
        registry
            .authorize(&restored.credential, None)
            .unwrap()
            .principal(),
        &principal
    );
    for (slug, document) in slugs.iter().zip(documents) {
        assert_eq!(json!(comb.get(&slug.parse().unwrap()).unwrap()), document);
    }
    check_replayed(
        &mut comb,
        &actor,
        &consolidate,
        &proposal_hash,
        &captured_slug,
    );
    drop(registry);
    drop(comb);
    refused_without_changes(root.path());
}

fn seed_mutations(
    comb: &mut Comb,
    actor: &Actor,
) -> (Consolidate, crate::values::ContentHash, crate::values::Slug) {
    let write = |id| Write::new(actor, &id, Some("synthetic-release")).unwrap();
    for slug in ["inbox/note", "review/proposal"] {
        comb.put_markdown(
            &slug.parse().unwrap(),
            "---\ntitle: writer evidence\n---\nwriter observation.",
            None,
            &write(slug.parse().unwrap()),
        )
        .unwrap();
    }
    let note: crate::values::Slug = "inbox/note".parse().unwrap();
    let target: crate::values::Slug = "concepts/writer".parse().unwrap();
    let consolidate = Consolidate {
        note: note.clone(),
        target: target.clone(),
        note_hash: comb.get(&note).unwrap().hash,
        target_hash: comb.get(&target).unwrap().hash,
        summary: "The writer retains messages. [evidence:new]".into(),
        recopy: false,
    };
    let consolidated = comb
        .consolidate(&consolidate, &write("fold".parse().unwrap()))
        .unwrap();
    assert!(!consolidated.note.replayed && !consolidated.target.replayed);
    let proposal = "review/proposal".parse().unwrap();
    let proposal_hash = comb.get(&proposal).unwrap().hash;
    let decided = comb
        .decide(
            &proposal,
            &proposal_hash,
            crate::page::Decision::Reject,
            "Needs evidence",
            &write("decide".parse().unwrap())
                .review_for_decision(true)
                .unwrap(),
        )
        .unwrap();
    assert!(!decided.replayed);
    let captured = comb
        .capture("writer capture", &write("capture".parse().unwrap()))
        .unwrap();
    (consolidate, proposal_hash, captured.slug)
}

fn seed_credential(comb: &Comb, root: &Path) -> (PathBuf, crate::values::Principal) {
    let registry = Registry::open(&comb.disk, comb.health.clone()).unwrap();
    let credential = registry
        .create(Grant::Client {
            name: "Corpus reader".into(),
            reviewer: false,
        })
        .unwrap();
    let connection = Connection {
        format: hive_input::compatibility::Exact,
        socket: root.join("service.sock"),
        credential,
    };
    let connection_path = root.join("connection.json");
    connection.write(&connection_path).unwrap();
    let principal = registry
        .authorize(&connection.credential, None)
        .unwrap()
        .principal()
        .clone();
    (connection_path, principal)
}

fn refused_without_changes(root: &Path) {
    fs::write(root.join(".comb/format"), b"{\"format\":4294967295}\n").unwrap();
    let before = image(root);
    for dry_run in [true, false] {
        assert_eq!(
            Comb::upgrade(root, dry_run).err().unwrap().code,
            crate::ErrorCode::NewerFormat
        );
        assert_eq!(image(root), before);
    }
}

fn check_replayed(
    comb: &mut Comb,
    actor: &Actor,
    consolidate: &Consolidate,
    proposal_hash: &crate::values::ContentHash,
    captured_slug: &crate::values::Slug,
) {
    let write = |id| Write::new(actor, &id, Some("synthetic-release")).unwrap();
    let proposal = "review/proposal".parse().unwrap();
    assert!(
        comb.consolidate(consolidate, &write("fold".parse().unwrap()))
            .unwrap()
            .target
            .replayed
    );
    assert!(
        comb.decide(
            &proposal,
            proposal_hash,
            crate::page::Decision::Reject,
            "Needs evidence",
            &write("decide".parse().unwrap())
                .review_for_decision(true)
                .unwrap()
        )
        .unwrap()
        .replayed
    );
    let replay = comb
        .capture("writer capture", &write("capture".parse().unwrap()))
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(&replay.slug, captured_slug);
}
