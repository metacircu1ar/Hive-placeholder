use super::*;
use crate::runtime::attachments::{Capture, Source};
mod removal;

#[test]
fn last_reference_queues_deletion_reuse_clears_it_and_recovery_removes_orphans() {
    let temp = crate::runtime::test_directory();
    let private = temp.path().join("state");
    state::directory(&private).unwrap();
    let store = Store::open(&private.join("db")).unwrap();
    for id in ["a", "b"] {
        store.save(&agent(id, None)).unwrap();
    }
    let source = temp.path().join("report.log");
    fs::write(&source, "snapshot").unwrap();
    let mut capture = Capture::new(store.attachments_dir.clone(), private.clone()).unwrap();
    let items = capture.message(std::slice::from_ref(&source)).unwrap();
    capture.publish().unwrap();
    let Source::Snapshot { path, .. } = &items[0].source else {
        panic!()
    };
    for id in ["a", "b"] {
        store.enqueue(id, "human", id, "work").unwrap();
        store.attach(id, &items).unwrap();
    }
    assert_eq!(store.attachment_bytes().unwrap(), 8);
    assert_eq!(
        store.next_message("a").unwrap().unwrap().attachments.len(),
        1
    );
    assert_eq!(
        store.messages("a", None, 10).unwrap()["messages"][0]["attachments"][0]["bytes"],
        8
    );
    store
        .execute_cached("DELETE FROM messages WHERE id='a'", [])
        .unwrap();
    assert!(store.pending_attachments().unwrap().1.is_empty());
    store
        .atomic(|s| {
            s.execute_cached("DELETE FROM messages WHERE id='b'", [])?;
            Ok(())
        })
        .unwrap();
    assert_eq!(store.pending_attachments().unwrap().1.len(), 1);
    assert!(path.exists()); // SQL cleanup never unlinks before COMMIT.
    store.enqueue("reuse", "human", "a", "again").unwrap();
    store.attach("reuse", &items).unwrap();
    assert!(store.pending_attachments().unwrap().1.is_empty());
    store.recover_attachments().unwrap();
    assert!(path.exists());
    store
        .execute_cached("DELETE FROM messages WHERE id='reuse'", [])
        .unwrap();
    store.recover_attachments().unwrap();
    assert!(!path.exists());
    assert_eq!(store.attachment_bytes().unwrap(), 0);
    let mut orphan = Capture::new(store.attachments_dir.clone(), private).unwrap();
    orphan.message(&[source]).unwrap();
    orphan.publish().unwrap();
    assert!(path.exists());
    store.recover_attachments().unwrap();
    assert!(!path.exists());
}

#[test]
fn rollback_retains_refs_and_durable_cleanup_outbox_survives_reopen() {
    let temp = crate::runtime::test_directory();
    let db = temp.path().join("db");
    let store = Store::open(&db).unwrap();
    store.save(&agent("a", None)).unwrap();
    let workspace = crate::runtime::test_directory();
    let source = workspace.path().join("input");
    fs::write(&source, "durable").unwrap();
    let mut capture = Capture::new(store.attachments_dir.clone(), temp.path().to_owned()).unwrap();
    let items = capture.message(&[source]).unwrap();
    capture.publish().unwrap();
    store.enqueue("m", "human", "a", "work").unwrap();
    store.attach("m", &items).unwrap();
    let failed: Result<()> = store.atomic(|s| {
        s.execute_cached("DELETE FROM messages WHERE id='m'", [])?;
        Err(Error::invalid_request("rollback"))
    });
    assert!(failed.is_err());
    assert!(store.pending_attachments().unwrap().1.is_empty());
    assert_eq!(store.attachments("m").unwrap().len(), 1);
    store
        .atomic(|s| {
            s.execute_cached("DELETE FROM messages WHERE id='m'", [])?;
            Ok(())
        })
        .unwrap();
    drop(store);
    let store = Store::open(&db).unwrap();
    assert_eq!(store.pending_attachments().unwrap().1.len(), 1);
    store.recover_attachments().unwrap();
    assert!(store.pending_attachments().unwrap().1.is_empty());
}

use crate::runtime::{api::RequestEnvelope, hub::Caller, test_support::WriterGate};
use std::{sync::Arc, time::Duration};

#[tokio::test]
async fn cancelled_reuse_retains_publication_gate_through_commit_before_cleanup() {
    let runtime = crate::runtime::test_support::RuntimeFixture::new();
    let path = runtime.temp.path().join("shared-file");
    std::fs::write(&path, "retained").unwrap();
    let accepted = runtime
        .hub
        .dispatch(
            Caller::Human,
            "original".into(),
            RequestEnvelope::new(
                json!({"op":"send","target":"agent","body":"work","attachments":[path]}),
            ),
        )
        .await
        .unwrap();
    let id = accepted["message_id"].as_str().unwrap().to_owned();
    let items = runtime
        .hub
        .with(move |core| {
            let items = core.store().attachments(&id)?;
            core.store()
                .sql()
                .execute("DELETE FROM messages WHERE id=?1", [&id])?;
            Ok(items)
        })
        .await
        .unwrap();
    let Source::Snapshot { path: stored, .. } = &items[0].source else {
        panic!()
    };
    let stored = stored.clone();
    let gate = Arc::new(runtime.hub.attachment_gate.clone().lock_owned().await);
    let (entered, started) = tokio::sync::oneshot::channel();
    let (release, blocked) = WriterGate::new();
    let writer = runtime.hub.clone();
    let caller = tokio::spawn(async move {
        writer
            .with_retained(gate, move |core| {
                core.store()
                    .enqueue("replacement", "human", "agent", "work")?;
                core.store().attach("replacement", &items)?;
                entered.send(()).unwrap();
                blocked.recv().unwrap();
                Ok(())
            })
            .await
    });
    started.await.unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    let cleanup = runtime.hub.clone();
    let mut cleaner = tokio::spawn(async move { cleanup.cleanup_attachments().await });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut cleaner)
            .await
            .is_err()
    );
    assert!(stored.exists());
    release.release();
    assert!(!cleaner.await.unwrap().unwrap());
    assert_eq!(std::fs::read(stored).unwrap(), b"retained");
    runtime
        .hub
        .with(|core| {
            assert_eq!(core.store().attachments("replacement")?.len(), 1);
            assert!(core.store().pending_attachments()?.1.is_empty());
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}

#[test]
fn metadata_must_match_relational_ownership_and_content_key() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    store.save(&agent("a", None)).unwrap();
    let workspace = crate::runtime::test_directory();
    let source = workspace.path().join("input");
    fs::write(&source, "bytes").unwrap();
    let mut capture = Capture::new(store.attachments_dir.clone(), temp.path().to_owned()).unwrap();
    let items = capture.message(&[source]).unwrap();
    capture.publish().unwrap();
    store.enqueue("m", "human", "a", "work").unwrap();
    store.attach("m", &items).unwrap();
    let good = serde_json::to_value(&items[0]).unwrap();
    let mut wrong_size = good.clone();
    wrong_size["bytes"] = json!(999);
    let mut wrong_key = good.clone();
    wrong_key["source"]["key"] = json!("b".repeat(64));
    let mut wrong_digest = good.clone();
    wrong_digest["source"]["sha256"] = json!("c".repeat(64));
    let mut live = good.clone();
    live["source"] = json!({"kind":"live_path","path":"/tmp/live","reason":"file_size_limit",
        "observed":{"device":0,"inode":0,"bytes":5,"mtime_seconds":0,"mtime_nanoseconds":0,
        "ctime_seconds":0,"ctime_nanoseconds":0}});
    for bad in [wrong_size, wrong_key, wrong_digest, live] {
        store
            .execute_cached(
                "UPDATE message_attachments SET data=?1 WHERE message_id='m'",
                [bad.to_string()],
            )
            .unwrap();
        assert!(store.attachments("m").is_err());
    }
    store
        .execute_cached(
            "UPDATE message_attachments SET data=?1,snapshot_key=NULL WHERE message_id='m'",
            [good.to_string()],
        )
        .unwrap();
    assert!(store.attachments("m").is_err());
}

#[test]
fn scheduled_promotion_and_quarantine_preserve_snapshot_and_live_attachments() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    store.save(&agent("a", None)).unwrap();
    let workspace = crate::runtime::test_directory();
    let small = workspace.path().join("small");
    let large = workspace.path().join("large");
    fs::write(&small, "snapshot").unwrap();
    fs::File::create(&large)
        .unwrap()
        .set_len(crate::runtime::attachments::FILE_BYTES + 1)
        .unwrap();
    let mut capture = Capture::new(store.attachments_dir.clone(), temp.path().to_owned()).unwrap();
    let items = capture.message(&[small.clone(), large]).unwrap();
    capture.publish().unwrap();
    fs::remove_file(small).unwrap();
    for id in ["promote", "quarantine"] {
        store
            .enqueue_scheduled(id, "human", "a", "delayed", 100)
            .unwrap();
        store.attach(id, &items).unwrap();
    }
    for row in store.due_scheduled(100).unwrap() {
        store
            .atomic(|s| {
                if row.id == "promote" {
                    s.activate_scheduled(&row, MessageState::Accepted)?;
                } else {
                    s.quarantine_scheduled(&row, false)?;
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(store.attachments(&row.id).unwrap().len(), 2);
        assert!(store.pending_attachments().unwrap().1.is_empty());
    }
    let message = store.next_message("a").unwrap().unwrap();
    assert_eq!(message.attachments.len(), 2);
    assert!(message.input().contains("small"));
    assert!(message.input().contains("LIVE PATH"));
    assert_eq!(store.attachment_bytes().unwrap(), 8);
}

#[test]
fn native_image_charges_survive_retries_and_rollback_atomically() {
    use crate::runtime::attachments::{Delivery, ReferenceReason, images};
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    store.save(&agent("a", None)).unwrap();
    let workspace = crate::runtime::test_directory();
    let source = workspace.path().join("image.png");
    fs::write(
        &source,
        b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01",
    )
    .unwrap();
    let mut capture = Capture::new(store.attachments_dir.clone(), temp.path().to_owned()).unwrap();
    let items = capture.message(&vec![source; 10]).unwrap();
    capture.publish().unwrap();
    for id in ["m1", "m2", "overflow"] {
        store.enqueue(id, "human", "a", "images").unwrap();
        store.attach(id, &items).unwrap();
    }
    let failed: Result<()> = store.atomic(|s| {
        s.plan_message_images(&mut s.next_message("a")?.unwrap())?;
        Err(Error::invalid_request("rollback"))
    });
    assert!(failed.is_err());
    assert_eq!(store.image_usage("a").unwrap(), images::Usage::default());
    for n in [10, 20] {
        let mut message = store.next_message("a").unwrap().unwrap();
        store
            .atomic(|s| s.plan_message_images(&mut message))
            .unwrap();
        let charged = store.image_usage("a").unwrap();
        assert_eq!(charged.images, n);
        let mut replay = store.next_message("a").unwrap().unwrap();
        store
            .atomic(|s| s.plan_message_images(&mut replay))
            .unwrap();
        assert_eq!(store.image_usage("a").unwrap(), charged);
        store
            .message_state(&message, MessageState::Completed)
            .unwrap();
    }
    let mut overflow = store.next_message("a").unwrap().unwrap();
    store
        .atomic(|s| s.plan_message_images(&mut overflow))
        .unwrap();
    assert!(overflow.attachments.iter().all(|a| a.delivery
        == Delivery::Reference {
            reason: ReferenceReason::ConversationImageBudget
        }));
    assert_eq!(store.image_usage("a").unwrap().images, 20);
}
