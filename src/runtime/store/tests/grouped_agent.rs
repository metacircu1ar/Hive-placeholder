use super::*;
use crate::runtime::domain::{AuthCutoff, ProviderProcess, SteeringCapability};

#[test]
fn provider_records_accept_only_absent_reserved_or_complete_identity() {
    let base = serde_json::to_value(agent("a", None)).unwrap();
    let fields = [
        ("provider_id", json!("provider")),
        ("provider_pid", json!(123)),
        ("provider_spawn_before", json!(10)),
        ("provider_spawn_after", json!(11)),
    ];
    for mask in 0..16 {
        let mut record = base.clone();
        for (bit, (key, value)) in fields.iter().enumerate() {
            if mask & (1 << bit) != 0 {
                record[key] = value.clone();
            }
        }
        let parsed = serde_json::from_value::<Agent>(record.clone());
        assert_eq!(parsed.is_ok(), matches!(mask, 0 | 1 | 15), "{mask}");
        if let Ok(agent) = parsed {
            assert_eq!(serde_json::to_value(agent).unwrap(), record);
        }
    }
    for (field, _) in fields {
        let mut record = base.clone();
        record.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<Agent>(record).is_err(), "{field}");
    }
}

#[test]
fn cutoff_and_steering_records_reject_contradictory_evidence() {
    let base = serde_json::to_value(agent("a", None)).unwrap();
    for cutoff in [
        None,
        Some(AuthCutoff::Observed { turn: "t".into() }),
        Some(AuthCutoff::Bound {
            turn: "t".into(),
            message: "m".into(),
        }),
    ] {
        let mut agent = agent("a", None);
        agent.auth_cutoff = cutoff.clone();
        let parsed: Agent = serde_json::from_value(serde_json::to_value(agent).unwrap()).unwrap();
        assert_eq!(parsed.auth_cutoff, cutoff);
    }
    let mut orphan = base.clone();
    orphan["auth_cutoff_message"] = json!("m");
    assert!(serde_json::from_value::<Agent>(orphan).is_err());
    for field in ["steering_disabled_reason", "steering_proof_error_code"] {
        let mut record = base.clone();
        record["steering_supported"] = json!(true);
        record[field] = json!("opaque-provider-diagnostic");
        assert!(serde_json::from_value::<Agent>(record).is_err());
    }
    let mut agent = agent("a", None);
    agent.steering = SteeringCapability::Disabled {
        reason: Some("opaque-provider-reason".into()),
        error_code: Some("opaque-provider-code".into()),
    };
    let record = serde_json::to_value(&agent).unwrap();
    assert!(record.get("steering_supported").is_none());
    assert_eq!(
        serde_json::to_value(serde_json::from_value::<Agent>(record.clone()).unwrap()).unwrap(),
        record
    );
    agent.disable_steering();
    assert_eq!(agent.steering.reason(), Some("opaque-provider-reason"));
}

#[test]
fn current_agent_record_excludes_dead_fields_and_preserves_public_projection() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    let mut agent = agent("a", None);
    agent.provider_process = Some(ProviderProcess::Started {
        id: "p".into(),
        pid: 123,
        before: 10,
        after: 11,
    });
    store.save(&agent).unwrap();
    let stored = store.agent("a").unwrap();
    assert_eq!(stored.provider_process, agent.provider_process);
    let record = serde_json::to_value(&stored).unwrap();
    for field in ["graph_id", "api_key_turns"] {
        assert!(record.get(field).is_none());
        let mut obsolete = record.clone();
        obsolete[field] = Value::Null;
        assert!(serde_json::from_value::<Agent>(obsolete).is_err());
    }
    assert_eq!(crate::runtime::api::value(&stored), record);
    let page = store.agent_page(0, 10, None, None).unwrap();
    assert_eq!(page["agents"][0]["id"], "a");
    for field in ["graph_id", "api_key_turns"] {
        assert!(page["agents"][0].get(field).is_none());
    }
    let positional = Value::Array(record.as_object().unwrap().values().cloned().collect());
    assert!(serde_json::from_value::<Agent>(positional).is_err());
    store
        .connection
        .execute("UPDATE agents SET provider_id=NULL WHERE id='a'", [])
        .unwrap();
    assert!(matches!(store.agent("a"), Err(Error::Database { .. })));
}

#[test]
fn conversation_reset_preserves_task_identity_and_admitted_selection() {
    let mut agent = agent("a", Some("parent"));
    agent.turn_id = Some("turn".into());
    agent.turn_selection = Some(agent.desired_selection());
    agent.reserved_turn = Some(crate::runtime::domain::ReservedTurn {
        message_id: "m".into(),
        selection: agent.desired_selection(),
    });
    agent.provider_process = Some(ProviderProcess::Reserved { id: "p".into() });
    agent.auth_cutoff = Some(AuthCutoff::Observed {
        turn: "turn".into(),
    });
    agent.steering = SteeringCapability::Supported;
    agent.native_input_attempted = true;
    let binding = crate::state::Binding {
        id: uuid::Uuid::new_v4(),
        profile: agent.profile.clone(),
        ready: true,
        directory: "profile".into(),
    };
    agent.reset_conversation(&binding);
    assert_eq!(agent.id, "a");
    assert_eq!(agent.parent_id.as_deref(), Some("parent"));
    assert_eq!(agent.reserved_turn.as_ref().unwrap().message_id, "m");
    assert_eq!(agent.auth_generation, binding.id);
    assert!(agent.conversation_clean);
    assert!(!agent.native_input_attempted);
    assert!(agent.thread_id.is_none());
    assert!(agent.turn_id.is_none() && agent.turn_selection.is_none());
    assert!(agent.provider_process.is_none() && agent.auth_cutoff.is_none());
    assert!(!agent.steering.supported());
}
