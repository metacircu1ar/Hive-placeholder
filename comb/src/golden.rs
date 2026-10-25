//! Permanent B0 contracts: update only for an explicitly approved format change.
use crate::{Actor, Page, page};
use serde::Serialize;
use std::path::Path;

mod receipts;
mod retrieval;
mod spaces;

pub(crate) const AT: &str = "2026-01-02T03:04:05.125+02:00";
pub(crate) const ID: &str = "12345678-1234-4234-8234-123456789abc";

pub(crate) fn check(name: &str, actual: impl AsRef<[u8]>) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/contracts")
        .join(name);
    if std::env::var_os("COMB_RECORD_GOLDENS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        atomic_fs::Publisher::new(
            (hive_setup_support::names::PUBLICATION_OPTIONS).with_mode(0o644),
        )
        .write(&path, actual.as_ref())
        .unwrap();
    }
    assert_eq!(std::fs::read(&path).unwrap(), actual.as_ref(), "{name}");
}

pub(crate) fn json(name: &str, actual: &impl Serialize) {
    check(name, serde_json::to_vec_pretty(actual).unwrap());
}

// Fixtures model persisted and forged claims through the same raw record decoder.
pub(crate) fn edit_record<T: Serialize + serde::de::DeserializeOwned>(
    record: &mut T,
    edit: impl FnOnce(&mut serde_json::Value),
) {
    let mut raw = serde_json::to_value(&*record).unwrap();
    edit(&mut raw);
    *record = serde_json::from_value(raw).unwrap();
}

pub(crate) fn actors() -> Vec<Actor> {
    vec![
        Actor::Human,
        Actor::Client {
            id: ID.into(),
            name: "A quoted \"client\"".into(),
        },
        Actor::External,
        Actor::Agent {
            bridge: "bridge".into(),
            id: "writer".into(),
            role: Some("reviewer".into()),
            execution: "execution".into(),
            turn: "turn".into(),
        },
        Actor::Collector {
            bridge: "bridge".into(),
        },
    ]
}

pub(crate) fn page(actor: Actor) -> Page {
    let mut page = Page::parse(
        "---\ntitle: 'Trust: 日本語'\ntype: note\ntags: [rust, \
        evidence]\naliases: [Knowledge]\n---\n# Summary\nA summary with [[concepts/other]] \
        and [a link](../decisions/review.md).\n",
    )
    .unwrap();
    edit_record(&mut page.metadata, |raw| {
        raw["author"] = serde_json::json!(actor);
        raw["created_at"] = serde_json::json!(AT);
        raw["updated_at"] = serde_json::json!(AT);
    });
    let ids = [
        ID.to_owned(),
        ID.to_uppercase(),
        ID.replace('-', ""),
        format!("urn:uuid:{ID}"),
        format!("{{{ID}}}"),
    ];
    page.timeline = actors()
        .into_iter()
        .zip(ids)
        .enumerate()
        .map(|(i, (actor, id))| page::Entry {
            metadata: serde_json::from_value(serde_json::json!({
                "id":id, "at":AT, "source":format!("fixture:<source-{i}>"),
                "actor":actor, "run":"run-α", "consolidation":null
            }))
            .unwrap(),
            text: format!("Evidence {i}: café, 日本語, <angle> and `code`."),
        })
        .collect();
    let content = page::ContentWitness::new(&page).unwrap();
    edit_record(&mut page, |raw| {
        raw["timeline"][0]["metadata"]["consolidation"] =
            serde_json::json!(page::ConsolidationWitness {
                source: "inbox/source".into(),
                source_hash: "ab".repeat(32),
                content: content.clone(),
                recopy: false,
            });
        raw["metadata"]["consolidated_into"] = serde_json::json!([page::ConsolidationCheckpoint {
            target: "concepts/target".into(),
            content: content.clone(),
            target_content: content,
            target_entry: ID.into(),
            by: Actor::Human,
            run: "curate".into(),
        }]);
    });
    page.promote(Actor::Human).unwrap();
    edit_record(&mut page.metadata, |raw| {
        raw["promotion"]["at"] = serde_json::json!(AT);
        raw["promotion"]["evidence"]["at"] = serde_json::json!(AT);
    });
    page.decide(page::ProposalDecision {
        decision: page::Decision::Accept,
        reason: "Reviewed <evidence>.".into(),
        by: Actor::Human,
        at: AT.into(),
    });
    page
}

#[test]
fn page_encodings_and_trust_digests_are_stable() {
    for (i, actor) in actors().into_iter().enumerate() {
        let page = page(actor);
        check(&format!("page-{i}.md"), page.render().unwrap());
        check(
            &format!("page-{i}.json"),
            serde_json::to_vec(&page).unwrap(),
        );
        json(
            &format!("page-{i}-digests.json"),
            &(
                page.compiled_digest().unwrap(),
                page::ContentWitness::new(&page).unwrap(),
                page.metadata.owned().promotion().unwrap(),
            ),
        );
        let restored = Page::parse(&page.render().unwrap()).unwrap();
        assert_eq!(
            serde_json::to_vec(&restored).unwrap(),
            serde_json::to_vec(&page).unwrap()
        );
        assert!(restored.verified());
        assert_eq!(restored.reviewed_start(), Some(0));
    }
}

#[test]
fn public_error_conversion_bytes_are_stable() {
    let errors = [
        crate::Error::from(std::io::Error::other("fixture filesystem diagnostic")),
        crate::Error::from(rusqlite::Error::InvalidQuery),
        crate::Error::from(serde_json::from_str::<Page>("{").unwrap_err()),
        crate::Error::from(serde_yaml_ng::from_str::<Page>("[").unwrap_err()),
    ];
    json("error-conversions.json", &errors);
}
