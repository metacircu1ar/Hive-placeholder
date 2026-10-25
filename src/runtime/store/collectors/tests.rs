use super::*;
use crate::runtime::{
    collectors::{Admin, Disposition, Evidence, EvidenceKind},
    domain::EventKind,
    hub::Caller,
    test_support::{AgentBuilder, TestStore},
};
use serde_json::json;
fn setup() -> TestStore {
    let store = TestStore::new();
    store
        .configure_collectors(
            &BTreeMap::from([(
                "shared".into(),
                CollectorConfig {
                    credential_file: "/tmp/unused-collector.json".into(),
                    enabled: true,
                    sources: [
                        CollectionSource::Correction,
                        CollectionSource::Review,
                        CollectionSource::TaskCompleted,
                        CollectionSource::TurnFailed,
                        CollectionSource::ApprovalAnswer,
                    ]
                    .into(),
                },
            )]),
            &BTreeMap::from([("shared".into(), "comb".into())]),
        )
        .unwrap();
    store
}
fn evidence() -> Evidence {
    Evidence {
        kind: EvidenceKind::Correction,
        summary: "Public measurement: contention increased.".into(),
        files: vec!["src/writer.rs".into()],
        commit: None,
        verdict: None,
    }
}
fn destination(store: &Store) -> Destination {
    store
        .execute_cached("UPDATE comb_collectors SET retry_at = 0", [])
        .unwrap();
    store.collector_destinations().unwrap().remove(0)
}
#[test]
fn outbox_is_atomic_survives_event_removal_and_has_installation_scoped_keys() {
    let store = setup();
    let first = store.submit_evidence(&Caller::Human, &evidence()).unwrap();
    assert_eq!(first.destinations["shared"], Disposition::Queued);
    let result: Result<()> = store.atomic(|store| {
        store.submit_evidence(&Caller::Human, &evidence())?;
        Err(Error::internal_error("test rollback"))
    });
    assert!(result.is_err());
    store.execute_cached("DELETE FROM events", []).unwrap();
    assert_eq!(store.collector_status().unwrap()[0].pending, 1);
    let item = store
        .claim_collection(&destination(&store), "comb", "key")
        .unwrap()
        .unwrap();
    let other = setup();
    other.submit_evidence(&Caller::Human, &evidence()).unwrap();
    let other_item = other
        .claim_collection(&destination(&other), "comb", "key")
        .unwrap()
        .unwrap();
    assert_ne!(item.id, other_item.id);
    let path = store.path();
    let reopened = Store::open(&path).unwrap();
    let retry = reopened
        .claim_collection(&destination(&reopened), "comb", "key")
        .unwrap()
        .unwrap();
    assert_eq!(retry.id, item.id);
    assert_eq!(retry.generation, item.generation);
    assert_eq!(retry.body, item.body);
}
#[test]
fn key_rotation_delivers_fresh_work_and_fences_released_attempts() {
    let store = setup();
    store.submit_evidence(&Caller::Human, &evidence()).unwrap();
    let old = store
        .claim_collection(&destination(&store), "comb", "old-key")
        .unwrap()
        .unwrap();
    for _ in 0..100 {
        store.submit_evidence(&Caller::Human, &evidence()).unwrap();
    }
    for _ in 0..100 {
        let item = store
            .claim_collection(&destination(&store), "comb", "new-key")
            .unwrap()
            .unwrap();
        assert_ne!(item.id, old.id);
        store.finish_collection(&item, None).unwrap();
    }
    assert!(
        store
            .claim_collection(&destination(&store), "comb", "new-key")
            .unwrap()
            .is_none()
    );
    let status = store.collector_status().unwrap().remove(0);
    assert_eq!((status.pending, status.uncertain), (1, 1));
    assert_eq!(
        status.last_error.as_deref(),
        Some("uncertain_principal_changed")
    );
    store
        .collector_admin(&Admin {
            action: "release_uncertain".to_owned().into(),
            destination: "shared".to_owned().into(),
            item: old.id.clone().into(),
            ..Default::default()
        })
        .unwrap();
    store.finish_collection(&old, None).unwrap();
    let new = store
        .claim_collection(&destination(&store), "comb", "new-key")
        .unwrap()
        .unwrap();
    assert_eq!(new.id, old.id);
    assert_eq!(new.generation, old.generation + 1);
    store.finish_collection(&old, Some("late_error")).unwrap();
    assert_eq!(store.collector_status().unwrap()[0].pending, 1);
    store.finish_collection(&new, None).unwrap();
    assert_eq!(store.collector_status().unwrap()[0].pending, 0);
}
#[test]
fn identity_change_pauses_and_removed_configuration_still_delivers() {
    let store = setup();
    store.submit_evidence(&Caller::Human, &evidence()).unwrap();
    let old = store
        .claim_collection(&destination(&store), "comb", "key")
        .unwrap()
        .unwrap();
    store.submit_evidence(&Caller::Human, &evidence()).unwrap();
    assert!(
        store
            .claim_collection(&destination(&store), "different", "key")
            .unwrap()
            .is_none()
    );
    assert!(store.collector_status().unwrap()[0].paused);
    let uuid = uuid::Uuid::new_v4().to_string();
    store
        .collector_admin(&Admin {
            action: "rebind".to_owned().into(),
            destination: "shared".to_owned().into(),
            comb_id: uuid.clone().into(),
            ..Default::default()
        })
        .unwrap();
    store
        .configure_collectors(&BTreeMap::new(), &BTreeMap::new())
        .unwrap();
    let status = store.collector_status().unwrap().remove(0);
    assert!(!status.enabled);
    assert_eq!(status.pending, 2);
    let item = store
        .claim_collection(&destination(&store), &uuid, "new-key")
        .unwrap()
        .unwrap();
    assert_ne!(item.id, old.id);
    store.finish_collection(&item, None).unwrap();
    store
        .collector_admin(&Admin {
            action: "discard".to_owned().into(),
            destination: "shared".to_owned().into(),
            ..Default::default()
        })
        .unwrap();
    store.finish_collection(&old, None).unwrap();
    assert_eq!(store.collector_status().unwrap()[0].pending, 0);
}
#[test]
fn overflow_is_visible_and_gap_queues_on_ack_even_when_disabled() {
    let store = setup();
    store
        .atomic(|store| {
            for _ in 0..1024 {
                assert_eq!(
                    store
                        .submit_evidence(&Caller::Human, &evidence())?
                        .destinations["shared"],
                    Disposition::Queued
                );
            }
            for _ in 0..3 {
                assert_eq!(
                    store
                        .submit_evidence(&Caller::Human, &evidence())?
                        .destinations["shared"],
                    Disposition::Full
                );
            }
            Ok(())
        })
        .unwrap();
    let status = store.collector_status().unwrap().remove(0);
    assert_eq!((status.pending, status.gap_count), (1024, 3));
    let warnings: i64 = store
        .sql()
        .cached()
        .query_row(
            "SELECT count(*) FROM events WHERE kind = 'collector_overflow'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(warnings, 1);
    store
        .configure_collectors(&BTreeMap::new(), &BTreeMap::new())
        .unwrap();
    let item = store
        .claim_collection(&destination(&store), "comb", "key")
        .unwrap()
        .unwrap();
    store.finish_collection(&item, None).unwrap();
    let status = store.collector_status().unwrap().remove(0);
    assert_eq!((status.pending, status.gap_count), (1024, 0));
    let gap: String = store
        .sql()
        .cached()
        .query_row(
            "SELECT body FROM comb_outbox ORDER BY sequence DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&gap).unwrap()["skipped_count"],
        3
    );
}
#[test]
fn automatic_evidence_excludes_arbitrary_provider_and_completion_text() {
    let store = setup();
    let agent = AgentBuilder::new("agent").build();
    store.save(&agent).unwrap();
    store
        .event(
            &agent,
            EventKind::TaskCompleted,
            json!({"result":"PRIVATE /home/secret password"}),
        )
        .unwrap();
    store
        .event(
            &agent,
            EventKind::ExecutionFailed,
            json!({"error":{"code":"invalid_request","message":"PRIVATE error text"}}),
        )
        .unwrap();
    store
        .event(
            &agent,
            EventKind::TurnFinished,
            json!({"status":"PRIVATE unknown status"}),
        )
        .unwrap();
    store
        .event(
            &agent,
            EventKind::ApprovalClosed,
            json!({"action":"accept","response_message":"PRIVATE answer"}),
        )
        .unwrap();
    let bodies: Vec<String> = store
        .sql()
        .cached()
        .query_column("SELECT body FROM comb_outbox", [])
        .unwrap();
    assert_eq!(bodies.len(), 3);
    for body in bodies {
        assert!(!body.contains("PRIVATE"), "{body}");
    }
    let mut explicit = evidence();
    for summary in [
        "[private] token",
        "<!-- private --> evidence",
        "<private>path</private>",
    ] {
        explicit.summary = summary.into();
        assert!(store.submit_evidence(&Caller::Human, &explicit).is_err());
    }
    explicit = evidence();
    explicit.files = vec!["/home/secret".into()];
    assert!(store.submit_evidence(&Caller::Human, &explicit).is_err());
}

#[test]
fn first_delivery_cannot_redirect_previously_admitted_evidence() {
    let store = setup();
    store.submit_evidence(&Caller::Human, &evidence()).unwrap();
    assert!(
        store
            .claim_collection(&destination(&store), "different-comb", "new-key")
            .unwrap()
            .is_none()
    );
    assert!(store.collector_status().unwrap()[0].paused);
    assert_eq!(store.collector_status().unwrap()[0].pending, 1);
    let unbound = TestStore::new();
    let config = BTreeMap::from([(
        "shared".into(),
        CollectorConfig {
            credential_file: "/tmp/unreadable.json".into(),
            sources: [CollectionSource::Correction].into(),
            enabled: true,
        },
    )]);
    unbound
        .configure_collectors(&config, &BTreeMap::new())
        .unwrap();
    unbound
        .submit_evidence(&Caller::Human, &evidence())
        .unwrap();
    assert!(unbound.collector_destinations().unwrap().is_empty());
    unbound
        .configure_collectors(
            &config,
            &BTreeMap::from([("shared".into(), "replacement".into())]),
        )
        .unwrap();
    assert!(
        unbound.collector_destinations().unwrap().is_empty(),
        "restart with a readable replacement does not bind retained work"
    );
}
#[test]
fn automatic_terminal_and_approval_records_keep_the_original_actor_and_turn() {
    let store = setup();
    let agent = AgentBuilder::new("subject").build();
    store.save(&agent).unwrap();
    for kind in [
        EventKind::TurnFinished,
        EventKind::ExecutionFailed,
        EventKind::TaskCompleted,
    ] {
        store.event(&agent,kind,json!({"turn_id":"original-turn","status":"failed","error":{"code":"invalid_request","message":"omit"},"result":"omit"})).unwrap();
    }
    store.event(&agent,EventKind::ApprovalClosed,json!({"approval_id":"approval-id","action":"accept","origin":{"execution":"asking-execution","turn":"asking-turn"}})).unwrap();
    let bodies: Vec<String> = store
        .sql()
        .cached()
        .query_column("SELECT body FROM comb_outbox ORDER BY sequence", [])
        .unwrap();
    for body in &bodies[..3] {
        let record: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(record["record"]["turn"], "original-turn");
    }
    let approval: serde_json::Value = serde_json::from_str(&bodies[3]).unwrap();
    assert_eq!(approval["record"]["actor"], "human");
    assert_eq!(approval["record"]["subject_agent"], "subject");
    assert_eq!(approval["record"]["execution"], "asking-execution");
    assert_eq!(approval["record"]["turn"], "asking-turn");
}

#[test]
fn lost_capture_ack_replays_after_runtime_restart_and_comb_rebuild() {
    let store = setup();
    store.submit_evidence(&Caller::Human, &evidence()).unwrap();
    let item = store
        .claim_collection(&destination(&store), "comb", "key")
        .unwrap()
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut comb = comb::Comb::init(directory.path()).unwrap();
    let actor = comb_protocol::Actor::Collector {
        bridge: "key".into(),
    };
    let id = format!("{}-{}", item.id, item.generation).parse().unwrap();
    let write = comb::store::Write::new(&actor, &id, Some("hive-collector")).unwrap();
    let first = comb.capture(&item.body, &write).unwrap();
    drop(comb); // Acknowledgement was lost, so outbox remains pinned.
    let mut comb = comb::Comb::rebuild(directory.path()).unwrap();
    let reopened = Store::open(&store.path()).unwrap();
    let retry = reopened
        .claim_collection(&destination(&reopened), "comb", "key")
        .unwrap()
        .unwrap();
    assert_eq!(format!("{}-{}", retry.id, retry.generation), id.as_str());
    let replay = comb.capture(&retry.body, &write).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.slug, first.slug);
    assert_eq!(comb.stats().unwrap().inbox, 1);
    reopened.finish_collection(&retry, None).unwrap();
    assert_eq!(reopened.collector_status().unwrap()[0].pending, 0);
}

#[test]
fn invalid_evidence_never_creates_an_event_or_outbox_item() {
    let store = setup();
    let mut value = evidence();
    value.summary = "<PRIVATE SECRET_CONTENT".into();
    let error = store.submit_evidence(&Caller::Human, &value).unwrap_err();
    assert_eq!(serde_json::to_value(error).unwrap()["field"], "/summary");
    value.summary = "Public correction.".into();
    value.files = vec!["../private".into()];
    assert!(store.submit_evidence(&Caller::Human, &value).is_err());
    let count: i64 = store
        .connection
        .query_row(
            "SELECT count(*) FROM events WHERE kind = 'evidence_submitted'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(store.collector_status().unwrap()[0].pending, 0);
    value.files.clear();
    store.submit_evidence(&Caller::Human, &value).unwrap();
    assert_eq!(store.collector_status().unwrap()[0].pending, 1);
}
