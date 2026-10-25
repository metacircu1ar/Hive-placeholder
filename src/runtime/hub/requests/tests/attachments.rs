use super::*;
use crate::runtime::attachments::Source;

#[tokio::test]
async fn publication_cleanup_failure_keeps_committed_reply_and_primary_request_error() {
    use crate::runtime::attachments::{Publication, Snapshot};
    let runtime = configured().await;
    for target in ["agent", "unknown"] {
        let request =
            RequestEnvelope::new(json!({"op":"send","target":target,"body":"private-work-canary"}));
        let result = runtime
            .hub
            .dispatch(Caller::Human, target.into(), request.clone())
            .await;
        let original = result.clone();
        let publication = Publication {
            root: runtime.temp.path().join("missing-attachment-root"),
            files: vec![Snapshot {
                key: "invalid-snapshot-identity".into(),
                filename: "fixture".into(),
            }],
            guard: Arc::new(runtime.hub.attachment_gate.clone().lock_owned().await),
        };
        assert!(
            runtime
                .hub
                .cleanup_publication(publication.clone())
                .await
                .is_err()
        );
        let settled = runtime
            .hub
            .settle_publication(publication, result.map(Some))
            .await;
        match original {
            Ok(accepted) => {
                assert_eq!(settled.unwrap(), Some(accepted.clone()));
                // The accepted message and its replay record still exist even
                // though publication cleanup failed after the writer commit.
                let replay = runtime
                    .hub
                    .dispatch(Caller::Human, target.into(), request)
                    .await
                    .unwrap();
                assert_eq!(replay, accepted);
            }
            Err(primary) => assert_eq!(settled.unwrap_err().code(), primary.code()),
        }
    }
    runtime.finish();
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "Sequential replay and file-lifetime regression retains one request and its publication gate."
)]
async fn attachment_replay_precedes_filesystem_and_batch_replay_survives_deleted_source() {
    let runtime = configured().await;
    let path = runtime.temp.path().join("input.log");
    std::fs::write(&path, "original").unwrap();
    let request = json!({"op":"send","target":"agent","body":"work","attachments":[path]});
    let accepted = runtime
        .hub
        .dispatch(
            Caller::Human,
            "one".into(),
            RequestEnvelope::new(request.clone()),
        )
        .await
        .unwrap();
    let id = accepted["message_id"].as_str().unwrap().to_owned();
    let items = runtime
        .hub
        .with(move |core| core.store().attachments(&id))
        .await
        .unwrap();
    let Source::Snapshot { path: stored, .. } = &items[0].source else {
        panic!()
    };
    std::fs::write(&path, "changed").unwrap();
    assert_eq!(
        runtime
            .hub
            .dispatch(
                Caller::Human,
                "one".into(),
                RequestEnvelope::new(request.clone())
            )
            .await
            .unwrap(),
        accepted
    );
    std::fs::remove_file(&path).unwrap();
    let held = runtime.hub.attachment_gate.clone().lock_owned().await;
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(2),
            runtime.hub.dispatch(
                Caller::Human,
                "one".into(),
                RequestEnvelope::new(request.clone())
            )
        )
        .await
        .unwrap()
        .unwrap(),
        accepted
    );
    let mut item = request.clone();
    item["request_id"] = json!("one");
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        runtime.hub.dispatch(
            Caller::Human,
            "batch".into(),
            RequestEnvelope::new(json!({"op":"batch","operations":[item]})),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result["results"][0], accepted);
    drop(held);
    assert_eq!(std::fs::read(stored).unwrap(), b"original");
    assert_eq!(
        runtime
            .hub
            .dispatch(
                Caller::Human,
                "fresh".into(),
                RequestEnvelope::new(request.clone())
            )
            .await
            .unwrap_err()
            .code(),
        "attachment_invalid"
    );
    let mut changed = request;
    changed["attachments"] = json!([runtime.temp.path().join("other")]);
    assert_eq!(
        runtime
            .hub
            .dispatch(Caller::Human, "one".into(), RequestEnvelope::new(changed))
            .await
            .unwrap_err()
            .code(),
        "request_conflict"
    );
    runtime.finish();
}

#[tokio::test]
async fn failed_batch_acceptance_and_publication_leave_no_durable_references() {
    let runtime = configured().await;
    let path = runtime.temp.path().join("input");
    std::fs::write(&path, "data").unwrap();
    let result = runtime.hub.dispatch(Caller::Human,"batch".into(),RequestEnvelope::new(json!({"op":"batch","operations":[
        {"op":"send","request_id":"first","target":"agent","body":"first","attachments":[path]},
        {"op":"send","request_id":"bad","target":"unknown","body":"bad"}
    ]}))).await;
    assert!(result.is_err());
    runtime
        .hub
        .with(|core| {
            assert_eq!(core.store().attachment_bytes()?, 0);
            core.store().recover_attachments()?;
            Ok(())
        })
        .await
        .unwrap();
    // The ID was never committed and is reusable.
    let ok = runtime
        .hub
        .dispatch(
            Caller::Human,
            "first".into(),
            RequestEnvelope::new(
                json!({"op":"send","target":"agent","body":"retry","attachments":[path]}),
            ),
        )
        .await
        .unwrap();
    assert!(ok.get("message_id").is_some());
    runtime.finish();
}

#[tokio::test]
async fn empty_attachments_bypass_capture_and_publication_gate() {
    let runtime = configured().await;
    let held = runtime.hub.attachment_gate.clone().lock_owned().await;
    for (id, request) in [
        (
            "omitted",
            json!({"op":"send","target":"agent","body":"work"}),
        ),
        (
            "empty",
            json!({"op":"send","target":"agent","body":"work","attachments":[]}),
        ),
    ] {
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            runtime
                .hub
                .dispatch(Caller::Human, id.into(), RequestEnvelope::new(request)),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(result.get("message_id").is_some());
    }
    drop(held);
    runtime.finish();
}

#[tokio::test]
async fn failed_captures_preserve_batch_operation_order_without_publication() {
    let runtime = configured().await;
    let source = runtime.temp.path().join("input");
    std::fs::write(&source, "snapshot").unwrap();
    let missing = runtime.temp.path().join("first-missing");
    let other = runtime.temp.path().join("second-missing");
    for (target, expected) in [
        ("unknown", "unknown_agent"),
        ("agent", "attachment_invalid"),
    ] {
        let result = runtime.hub.dispatch(Caller::Human, target.into(), RequestEnvelope::new(json!({
            "op":"batch","operations":[
                {"request_id":"early","op":"send","target":target,"body":"work","attachments":[source]},
                {"request_id":"missing","op":"send","target":"agent","body":"work","attachments":[missing]},
                {"request_id":"other","op":"send","target":"agent","body":"work","attachments":[other]}
            ]}))).await.unwrap_err();
        assert_eq!(result.code(), expected);
        if expected == "attachment_invalid" {
            assert!(result.message().contains("first-missing"));
        }
        runtime
            .hub
            .with(|core| {
                assert_eq!(core.store().attachment_bytes()?, 0);
                assert!(!core.store().next_message("agent")?.unwrap().has_attachments);
                Ok(())
            })
            .await
            .unwrap();
    }
    runtime.finish();
}

#[tokio::test]
async fn inner_replay_after_capture_cannot_commit_unpublished_batch_snapshots() {
    let runtime = configured().await;
    let a = runtime.temp.path().join("a");
    let b = runtime.temp.path().join("b");
    std::fs::write(&a, "first").unwrap();
    std::fs::write(&b, "second").unwrap();
    let send = json!({"op":"send","target":"agent","body":"second","attachments":[b]});
    let mut inner = send.clone();
    inner["request_id"] = json!("concurrent");
    let batch = RequestEnvelope::new(json!({"op":"batch","operations":[
        {"request_id":"unpublished","op":"send","target":"agent","body":"first","attachments":[a]},inner]}));
    let captured_batch = batch.clone();
    let input = runtime
        .hub
        .with(move |core| {
            Ok(crate::runtime::preflight::request::RequestInput::capture(
                core,
                &Caller::Human,
                &captured_batch,
            ))
        })
        .await
        .unwrap();
    runtime
        .hub
        .dispatch(
            Caller::Human,
            "concurrent".into(),
            RequestEnvelope::new(send),
        )
        .await
        .unwrap();
    std::fs::remove_file(&b).unwrap();
    let facts = tokio::task::spawn_blocking(move || input.resolve())
        .await
        .unwrap();
    let result = runtime
        .hub
        .with(move |core| core.dispatch(&Caller::Human, "batch", &batch, &facts))
        .await;
    assert_eq!(result.unwrap_err().code(), "attachment_invalid");
    runtime
        .hub
        .with(|core| {
            assert_eq!(core.store().attachment_bytes()?, 6);
            assert!(
                core.store().messages("agent", None, 10)?["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|m| m["body"] != "first")
            );
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}

#[tokio::test]
async fn rejected_requests_clean_unindexed_files_without_restart_and_keep_live_dedup() {
    let runtime = configured().await;
    let path = runtime.temp.path().join("snapshot");
    std::fs::write(&path, "data").unwrap();
    for (id, request) in [
        (
            "bad-send",
            json!({"op":"send","target":"unknown","body":"work","attachments":[path]}),
        ),
        (
            "bad-batch",
            json!({"op":"batch","operations":[
            {"request_id":"new","op":"send","target":"agent","body":"work","attachments":[path]},
            {"request_id":"bad","op":"send","target":"unknown","body":"work"}]}),
        ),
    ] {
        assert!(
            runtime
                .hub
                .dispatch(Caller::Human, id.into(), RequestEnvelope::new(request))
                .await
                .is_err()
        );
        let root = runtime
            .hub
            .with(|core| Ok(core.store().attachments_dir.clone()))
            .await
            .unwrap();
        assert_eq!(std::fs::read_dir(root).unwrap().count(), 0);
    }
    let good = json!({"op":"send","target":"agent","body":"work","attachments":[path]});
    runtime
        .hub
        .dispatch(Caller::Human, "good".into(), RequestEnvelope::new(good))
        .await
        .unwrap();
    let bad = json!({"op":"send","target":"unknown","body":"work","attachments":[path]});
    assert!(
        runtime
            .hub
            .dispatch(Caller::Human, "bad-dedup".into(), RequestEnvelope::new(bad))
            .await
            .is_err()
    );
    runtime
        .hub
        .with(|core| {
            assert_eq!(core.store().attachment_bytes()?, 4);
            assert_eq!(
                std::fs::read_dir(&core.store().attachments_dir)
                    .unwrap()
                    .count(),
                1
            );
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}

#[tokio::test]
async fn cancelled_caller_still_finishes_rejected_publication_cleanup() {
    let runtime = configured().await;
    let path = runtime.temp.path().join("cancelled-file");
    std::fs::write(&path, "orphan").unwrap();
    let (entered, running) = tokio::sync::oneshot::channel();
    let (release, blocked) = WriterGate::new();
    let service = runtime.hub.preflight.clone();
    let held = tokio::spawn(async move { service.hold(entered, blocked).await });
    running.await.unwrap();
    let hub = runtime.hub.clone();
    let caller = tokio::spawn(async move {
        hub.dispatch(
            Caller::Human,
            "cancelled-send".into(),
            RequestEnvelope::new(json!({
            "op":"send","target":"unknown","body":"work","attachments":[path]})),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while runtime.hub.attachment_gate.try_lock().is_ok() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    release.release();
    held.await.unwrap().unwrap();
    let guard = tokio::time::timeout(Duration::from_secs(2), runtime.hub.attachment_gate.lock())
        .await
        .unwrap();
    let root = runtime
        .hub
        .with(|core| Ok(core.store().attachments_dir.clone()))
        .await
        .unwrap();
    assert_eq!(std::fs::read_dir(root).unwrap().count(), 0);
    drop(guard);
    // Releasing the publication gate precedes the detached admission task's
    // final return. Let it drop its Hub while fixture teardown joins the writer.
    tokio::task::spawn_blocking(move || runtime.finish())
        .await
        .unwrap();
}

#[cfg(not(feature = "comb"))]
#[tokio::test]
async fn core_bound_branch_rejection_never_waits_for_attachment_staging() {
    let runtime = configured().await;
    runtime
        .hub
        .with(|core| {
            let mut agent = core.store().agent("agent")?;
            agent.comb = Some(crate::config::CombBinding {
                credential_file: "/absent/comb-credential.json".into(),
                reviewer: false,
            });
            core.store().save(&agent)
        })
        .await
        .unwrap();
    // Holding this gate proves no attachment staging begins, without depending on
    // file access-time behavior or a slow filesystem.
    let guard = runtime.hub.attachment_gate.lock().await;
    let request = json!({"op":"spawn", "from_agent":"agent", "role":"test", "body":"branch", "attachments":["/absent/attachment"]});
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        runtime.hub.dispatch(
            Caller::Human,
            "branch".into(),
            RequestEnvelope::new(request.clone()),
        ),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(error.code(), "unsupported_operation");
    let mut item = request;
    item["request_id"] = json!("branch-in-batch");
    let batch = json!({"op":"batch", "operations":[{"op":"send","request_id":"bad-first","target":"unknown","body":"work"},item]});
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        runtime
            .hub
            .dispatch(Caller::Human, "batch".into(), RequestEnvelope::new(batch)),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(error.code(), "unknown_agent");
    drop(guard);
    runtime
        .hub
        .with(|core| {
            assert_eq!(core.store().attachment_bytes()?, 0);
            assert_eq!(
                std::fs::read_dir(&core.store().attachments_dir)
                    .unwrap()
                    .count(),
                0
            );
            Ok(())
        })
        .await
        .unwrap();
    runtime.finish();
}
