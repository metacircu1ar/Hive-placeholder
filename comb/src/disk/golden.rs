use super::*;
use crate::{Actor, golden};

fn identity() -> Identity {
    Identity {
        id: (uuid::Uuid::parse_str(golden::ID).unwrap()).into(),
        key: std::array::from_fn(|i| u8::try_from(i).unwrap()),
    }
}

pub(crate) fn seeded_comb(pages: &[(&str, Page)]) -> (tempfile::TempDir, crate::Comb) {
    let root = tempfile::tempdir().unwrap();
    let mut disk = Disk::acquire(root.path(), true).unwrap();
    disk.identity = identity();
    atomic(
        &disk.admin.join("identity.json"),
        &serde_json::to_vec(&disk.identity).unwrap(),
        &System,
    )
    .unwrap();
    for (slug, page) in pages {
        let mut page = page.clone();
        disk.seal(&slug.parse().unwrap(), &mut page).unwrap();
        disk.write(&slug.parse().unwrap(), Some(&page), None, None)
            .unwrap();
        disk.finish().unwrap();
    }
    drop(disk);
    let comb = crate::Comb::open(root.path()).unwrap();
    (root, comb)
}

#[test]
fn mac_input_and_seals_preserve_every_actor_encoding() {
    let root = tempfile::tempdir().unwrap();
    let mut disk = Disk::acquire(root.path(), true).unwrap();
    disk.identity = identity();
    for (i, actor) in golden::actors().into_iter().enumerate() {
        let mut page = golden::page(actor);
        golden::check(
            &format!("seal-input-{i}.json"),
            serde_json::to_vec(&(disk.id(), "concepts/trust", &page)).unwrap(),
        );
        disk.seal(&"concepts/trust".parse().unwrap(), &mut page)
            .unwrap();
        golden::json(&format!("seal-{i}.json"), &page.metadata.owned().seal());
        assert!(
            disk.valid_seal(&"concepts/trust".parse().unwrap(), &page)
                .unwrap()
        );
        assert!(
            !disk
                .valid_seal(&"concepts/other".parse().unwrap(), &page)
                .unwrap()
        );
    }
}

#[test]
fn authorship_changes_trust_but_not_content_identity() {
    let root = tempfile::tempdir().unwrap();
    let mut disk = Disk::acquire(root.path(), true).unwrap();
    disk.identity = identity();
    let mut original = golden::page(Actor::Human);
    disk.seal(&"concepts/trust".parse().unwrap(), &mut original)
        .unwrap();
    let mut edited = original.clone();
    golden::edit_record(&mut edited.timeline[1].metadata, |raw| {
        raw["actor"] = serde_json::json!(Actor::External);
        raw["run"] = serde_json::json!("changed-run");
    });
    assert_eq!(
        page::ContentWitness::new(&original).unwrap(),
        page::ContentWitness::new(&edited).unwrap()
    );
    assert!(
        !disk
            .valid_seal(&"concepts/trust".parse().unwrap(), &edited)
            .unwrap()
    );
    assert_eq!(edited.reviewed_start(), None);
    disk.seal(&"concepts/trust".parse().unwrap(), &mut edited)
        .unwrap();
    assert_ne!(
        original.metadata.owned().seal(),
        edited.metadata.owned().seal()
    );
    edited.promote(Actor::Human).unwrap();
    assert_ne!(
        original
            .metadata
            .owned()
            .promotion()
            .unwrap()
            .evidence
            .as_ref()
            .unwrap()
            .digest,
        edited
            .metadata
            .owned()
            .promotion()
            .unwrap()
            .evidence
            .as_ref()
            .unwrap()
            .digest
    );
}

#[test]
fn durable_record_bytes_come_from_the_real_write_path() {
    let root = tempfile::tempdir().unwrap();
    let mut disk = Disk::acquire(root.path(), true).unwrap();
    disk.identity = identity();
    golden::check("identity.json", serde_json::to_vec(&disk.identity).unwrap());
    golden::check("population.json", POPULATION);
    golden::json("service-key.json", &service_key(&disk.identity).unwrap());
    let mut page = golden::page(Actor::Human);
    disk.seal(&"concepts/trust".parse().unwrap(), &mut page)
        .unwrap();
    let receipt = page::Receipt {
        principal: Actor::Human.principal().unwrap(),
        request_id: "write".parse().unwrap(),
        fingerprint: "cd".repeat(32),
        operation: "put".into(),
        slug: "concepts/trust".parse().unwrap(),
    };
    disk.write(
        &"concepts/trust".parse().unwrap(),
        Some(&page),
        None,
        Some(receipt.clone()),
    )
    .unwrap();
    golden::json(
        "sidecar-paths.json",
        &(
            &receipt.principal,
            disk.head_path(&"concepts/trust".parse().unwrap())
                .strip_prefix(&disk.admin)
                .unwrap(),
            disk.receipt_path(receipt.principal.as_str(), receipt.request_id.as_str())
                .unwrap()
                .strip_prefix(&disk.admin)
                .unwrap(),
        ),
    );
    for (name, path) in [
        ("intent-create.json", disk.admin.join("intent.json")),
        (
            "head.json",
            disk.head_path(&"concepts/trust".parse().unwrap()),
        ),
        (
            "receipt.json",
            disk.receipt_path(receipt.principal.as_str(), receipt.request_id.as_str())
                .unwrap(),
        ),
    ] {
        golden::check(name, fs::read(path).unwrap());
    }
    disk.finish().unwrap();
    disk.write(
        &"concepts/trust".parse().unwrap(),
        None,
        Some(&ContentHash::digest(page.render().unwrap())),
        None,
    )
    .unwrap();
    golden::check(
        "intent-remove.json",
        fs::read(disk.admin.join("intent.json")).unwrap(),
    );
    disk.finish().unwrap();
}

#[test]
fn removal_records_and_replies_come_from_the_real_remove_path() {
    for interrupted in [false, true] {
        let (root, mut comb) = seeded_comb(&[("concepts/trust", golden::page(Actor::Human))]);
        let slug = "concepts/trust".parse().unwrap();
        let expected = comb.get(&slug).unwrap().hash;
        let write =
            crate::store::Write::new(&Actor::Human, &"remove".parse().unwrap(), None).unwrap();
        let recording = Arc::new(Recording::default());
        comb.disk.set_durability(recording.clone());
        if interrupted {
            recording.fail_finish_once();
        }
        let result = comb.remove(&slug, &expected, &write);
        if interrupted {
            assert!(result.unwrap_err().file_committed);
            golden::check(
                "removal-intent.json",
                fs::read(comb.disk.admin.join("intent.json")).unwrap(),
            );
        } else {
            let removed = result.unwrap();
            assert!(!removed.replayed);
            golden::json("removal-result.json", &removed);
        }
        golden::check(
            "removal-head.json",
            fs::read(comb.disk.head_path(&slug)).unwrap(),
        );
        golden::check(
            "removal-receipt.json",
            fs::read(
                comb.disk
                    .receipt_path(Actor::Human.principal().unwrap().as_str(), "remove")
                    .unwrap(),
            )
            .unwrap(),
        );
        drop(comb);
        let mut comb = crate::Comb::open(root.path()).unwrap();
        let replay = comb.remove(&slug, &expected, &write).unwrap();
        assert!(replay.replayed);
        golden::json("removal-replay.json", &replay);
    }
}
