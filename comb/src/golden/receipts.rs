use crate::{
    Actor, Comb, Page,
    disk::golden::seeded_comb,
    golden, page,
    store::{Consolidate, Write},
};

fn seed(title: &str) -> Page {
    let mut page = Page::parse(&format!("---\ntitle: {title}\n---\nOriginal content.\n")).unwrap();
    golden::edit_record(&mut page.metadata, |raw| {
        raw["created_at"] = serde_json::json!(golden::AT);
        raw["updated_at"] = serde_json::json!(golden::AT);
        raw["author"] = serde_json::json!(Actor::Human);
    });
    page
}

#[test]
fn page_mutations_preserve_the_original_receipt_identity() {
    let (root, mut comb) = seeded_comb(&[
        ("concepts/page", seed("Page")),
        ("review/proposal", seed("Proposal")),
        ("inbox/note", seed("Note")),
        ("concepts/target", seed("Target")),
    ]);
    let actor = Actor::Client {
        id: golden::ID.into(),
        name: "Writer".into(),
    };
    let write = |request_id| Write::new(&actor, &request_id, Some("run")).unwrap();
    let original = comb.get(&"concepts/page".parse().unwrap()).unwrap().hash;
    // Each expected hash below comes from deterministic seed bytes, never a clock.
    let proposal = comb.get(&"review/proposal".parse().unwrap()).unwrap().hash;
    comb.promote(
        &"concepts/page".parse().unwrap(),
        &original,
        &write("promote".parse().unwrap())
            .review_for_promotion(true)
            .unwrap(),
    )
    .unwrap();
    create_page_with_evidence(&mut comb, &actor);
    comb.capture("Captured note", &write("capture".parse().unwrap()))
        .unwrap();
    comb.decide(
        &"review/proposal".parse().unwrap(),
        &proposal,
        page::Decision::Reject,
        "Needs evidence",
        &write("decide".parse().unwrap())
            .review_for_decision(true)
            .unwrap(),
    )
    .unwrap();
    let request = Consolidate {
        note: "inbox/note".parse().unwrap(),
        target: "concepts/target".parse().unwrap(),
        note_hash: comb.get(&"inbox/note".parse().unwrap()).unwrap().hash,
        target_hash: comb.get(&"concepts/target".parse().unwrap()).unwrap().hash,
        summary: "Consolidated summary. [evidence:new]".into(),
        recopy: false,
    };
    comb.consolidate(&request, &write("consolidate".parse().unwrap()))
        .unwrap();
    assert_receipts(root.path());
}

fn create_page_with_evidence(comb: &mut Comb, actor: &Actor) {
    let write = |request_id| Write::new(actor, &request_id, Some("run")).unwrap();
    comb.put_markdown(
        &"concepts/new".parse().unwrap(),
        "---\ntitle: New\n---\nNew content.",
        None,
        &write("put".parse().unwrap()),
    )
    .unwrap();
    comb.append(
        &"concepts/new".parse().unwrap(),
        "Observation",
        "test:source",
        None,
        None,
        &write("append".parse().unwrap()),
    )
    .unwrap();
}

fn assert_receipts(root: &std::path::Path) {
    let mut receipts = std::fs::read_dir(root.join(".comb/receipts"))
        .unwrap()
        .map(|entry| {
            serde_json::from_slice::<page::Receipt>(&std::fs::read(entry.unwrap().path()).unwrap())
                .unwrap()
        })
        .collect::<Vec<_>>();
    receipts.sort_by(|a, b| a.request_id.cmp(&b.request_id));
    assert_eq!(receipts.len(), 7);
    golden::json("mutation-receipts.json", &receipts);
}
