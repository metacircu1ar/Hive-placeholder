use super::*;
use crate::runtime::domain::{EventKind, decode};

#[test]
fn event_constraint_matches_every_current_kind_and_rejects_unknown_kinds() {
    let constraint = format!(
        "CHECK(kindIN({}))",
        EventKind::VALUES
            .iter()
            .map(|kind| format!("'{kind}'"))
            .collect::<Vec<_>>()
            .join(",")
    );
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    let schema: String = store
        .connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='events'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        schema
            .chars()
            .filter(|c| !c.is_ascii_whitespace())
            .collect::<String>()
            .contains(&constraint)
    );
    let agent = agent("a", None);
    store.save(&agent).unwrap();
    for kind in EventKind::VALUES {
        store
            .event(&agent, EventKind::parse(kind).unwrap(), json!({}))
            .unwrap();
    }
    assert_eq!(
        store.events("a", 0, 200).unwrap()["events"]
            .as_array()
            .unwrap()
            .len(),
        EventKind::VALUES.len()
    );
    assert!(
        store
            .connection
            .execute(
                "INSERT INTO events(agent,at,kind,data) VALUES('a',1,'future','{}')",
                []
            )
            .is_err()
    );
    // Simulate on-disk corruption independently of the INSERT constraint.
    store
        .connection
        .pragma_update(None, "ignore_check_constraints", true)
        .unwrap();
    store
        .connection
        .execute("UPDATE events SET kind='future' WHERE seq=1", [])
        .unwrap();
    store
        .connection
        .pragma_update(None, "ignore_check_constraints", false)
        .unwrap();
    assert_eq!(
        store.events("a", 0, 200).unwrap_err().code(),
        "database_error"
    );
}

#[test]
fn agent_records_require_every_emitted_field_and_preserve_compact_omissions() {
    let agent = agent("a", None);
    let good = api::value(&agent);
    assert_eq!(api::value(decode::object::<_, Agent>(&good).unwrap()), good);
    for key in good.as_object().unwrap().keys() {
        let mut missing = good.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(
            decode::object::<_, Agent>(&missing).is_err(),
            "missing {key}"
        );
    }
    for key in [
        "recovery_fence",
        "redirect_pending",
        "instructions",
        "native_input_attempted",
        "steering_supported",
        "steering_disabled_reason",
        "steering_proof_error_code",
    ] {
        assert!(good.get(key).is_none(), "compact {key}");
    }
    let mut unknown = good.clone();
    unknown["extension"] = json!(true);
    assert!(decode::object::<_, Agent>(&unknown).is_err());
    let mut old = good.clone();
    old["profile"]["extension"] = json!(true);
    assert!(decode::object::<_, Agent>(&old).is_err());
    for field in [
        "state",
        "task_outcome",
        "lifetime",
        "sandbox",
        "branch_phase",
        "waiting_reason",
        "control",
    ] {
        for value in [json!("future"), json!({"idle":null})] {
            let mut corrupt = good.clone();
            corrupt[field] = value;
            assert!(decode::object::<_, Agent>(&corrupt).is_err(), "{field}");
        }
    }
    for field in [
        "reserved_turn",
        "session_selection",
        "last_finished_turn",
        "last_attempted_turn",
        "auth_switch_pending",
    ] {
        let mut corrupt = good.clone();
        corrupt[field] = json!([]);
        assert!(decode::object::<_, Agent>(&corrupt).is_err(), "{field}");
    }
    assert!(decode::from_str::<Agent>("[]").is_err());
}

#[test]
fn every_agent_read_validates_saved_fields_before_projecting_them() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    let mut child = agent("child", Some("parent"));
    child.lifetime = crate::config::Lifetime::WithParent;
    store.save(&agent("parent", None)).unwrap();
    store.save(&child).unwrap();
    for expression in [
        "json_remove(data,'$.created_at')",
        "json_set(data,'$.unknown',1)",
        "json_set(data,'$.sandbox','future')",
        "json_set(data,'$.profile',json('[]'))",
        "json_set(data,'$.model','mismatched')",
        "json_set(data,'$.effort','mismatched')",
        "json_set(data,'$.options_revision',-1)",
        "json_set(data,'$.applied_options_revision',-1)",
        "json_remove(data,'$.agent_options.storage')",
        "json_set(data,'$.agent_options.storage','future')",
    ] {
        store.save(&child).unwrap();
        store
            .connection
            .execute(
                &format!("UPDATE agents SET data={expression} WHERE id='child'"),
                [],
            )
            .unwrap();
        assert_eq!(store.agent("child").unwrap_err().code(), "database_error");
        assert_eq!(
            store.agent_page(0, 100, None, None).unwrap_err().code(),
            "database_error"
        );
        assert_eq!(
            store.children("parent").unwrap_err().code(),
            "database_error"
        );
    }
}

#[test]
fn agent_reads_reject_selection_identities_that_disagree_with_the_private_snapshot() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    let original = agent("a", None);
    for statement in [
        "UPDATE agents SET data=json_set(data,'$.model','different-model') WHERE id='a'",
        "UPDATE agents SET data=json_set(data,'$.effort','medium') WHERE id='a'",
    ] {
        store.save(&original).unwrap();
        store.connection.execute(statement, []).unwrap();
        assert_eq!(store.agent("a").unwrap_err().code(), "database_error");
    }
}

#[test]
fn distinct_historical_snapshots_cannot_reuse_the_desired_validation_proof() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    let original = agent("a", None);
    let mut good = api::value(original.desired_selection());
    let mut bad = good.clone();
    bad["options"]["values"]["codex.request_compression"] = json!("private-canary");
    bad["options"]["sources"]["codex.request_compression"] =
        json!({"source":"spawn","native_overlay":false});
    good.as_object_mut().unwrap().remove("options");
    for field in [
        "reserved_turn",
        "session_selection",
        "last_accepted_selection",
        "turn_selection",
    ] {
        for (selection, accepted) in [(&good, true), (&bad, false)] {
            store.save(&original).unwrap();
            let value = if field == "reserved_turn" {
                json!({"message_id":"m","selection":selection})
            } else {
                selection.clone()
            };
            store
                .connection
                .execute(
                    "UPDATE agents SET data=json_set(data,?1,json(?2)) WHERE id='a'",
                    params![format!("$.{field}"), value.to_string()],
                )
                .unwrap();
            let result = store.agent("a");
            if accepted {
                assert!(result.is_ok(), "{field}");
            } else {
                assert_eq!(result.unwrap_err().code(), "database_error", "{field}");
            }
        }
    }
}

#[test]
fn compact_selection_records_rehydrate_exactly_and_public_views_stay_complete() {
    type PublicAgent =
        Agent<crate::runtime::domain::CombCapability, Vec<String>, crate::options::OptionsView>;
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    let mut original = agent("a", None);
    let baseline = original.desired_selection();
    original.reserved_turn = Some(crate::runtime::domain::ReservedTurn {
        message_id: "m".into(),
        selection: baseline.clone(),
    });
    original.session_selection = Some(baseline.clone());
    original.last_accepted_selection = Some(baseline.clone());
    original.turn_selection = Some(baseline);
    let stored = api::value(&original);
    assert_eq!(stored["agent_options"]["storage"], "baseline_refs");
    assert!(stored["agent_options"].get("overrides").is_none());
    assert!(stored["agent_options"].get("baseline_configs").is_none());
    assert!(stored["agent_options"].get("config").is_none());
    for field in [
        "session_selection",
        "last_accepted_selection",
        "turn_selection",
    ] {
        assert!(stored[field].get("options").is_none());
    }
    assert!(
        stored["reserved_turn"]["selection"]
            .get("options")
            .is_none()
    );
    store.save(&original).unwrap();
    let restored = store.agent("a").unwrap();
    assert_eq!(restored.reserved_turn, original.reserved_turn);
    assert_eq!(restored.session_selection, original.session_selection);
    assert_eq!(
        restored.last_accepted_selection,
        original.last_accepted_selection
    );
    assert_eq!(restored.turn_selection, original.turn_selection);
    let public = api::value(restored.public());
    assert_eq!(
        public["session_selection"]["options"]["values"]["model"],
        "mock"
    );
    assert!(public["agent_options"].get("storage").is_none());
    let mut public_reference = public;
    public_reference["session_selection"]["options"] = json!("baseline");
    assert!(decode::object::<_, PublicAgent>(&public_reference).is_err());
    public_reference["session_selection"]
        .as_object_mut()
        .unwrap()
        .remove("options");
    assert!(decode::object::<_, PublicAgent>(&public_reference).is_err());
    for invalid in [
        json!("unknown"),
        json!("baseline"),
        json!(null),
        json!({"baseline":null}),
        json!([]),
        api::value(original.desired_selection().options),
    ] {
        let mut corrupt = stored.clone();
        corrupt["session_selection"]["options"] = invalid;
        assert!(decode::object::<_, Agent>(&corrupt).is_err());
    }
    let mut old = stored;
    old["agent_options"]
        .as_object_mut()
        .unwrap()
        .remove("storage");
    assert!(decode::object::<_, Agent>(&old).is_err());
}

#[test]
fn stored_references_keep_frozen_defaults_and_distinct_historical_settings() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    let mut original = agent("a", None);
    let frozen = original.desired_selection();
    original.session_selection = Some(frozen.clone());
    let mut distinct = frozen.clone();
    std::sync::Arc::make_mut(&mut distinct.options)
        .values
        .insert("codex.reasoning_summary".into(), json!("detailed"));
    std::sync::Arc::make_mut(&mut distinct.options)
        .sources
        .insert(
            "codex.reasoning_summary".into(),
            crate::options::OptionSource::plain("set_options"),
        );
    original.last_accepted_selection = Some(distinct.clone());
    original
        .agent_options
        .overrides
        .insert("effort".into(), json!("low"));
    original.effort = "low".into();
    let raw = api::value(&original);
    assert!(raw["session_selection"].get("options").is_none());
    assert!(raw["last_accepted_selection"]["options"].is_object());
    store.save(&original).unwrap();
    let restored = store.agent("a").unwrap();
    assert_eq!(restored.effort, "low");
    assert_eq!(restored.session_selection, Some(frozen));
    assert_eq!(restored.last_accepted_selection, Some(distinct));
}

#[test]
fn corrupted_replay_is_an_error_and_identity_conflicts_still_precede_decoding() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    for response in [
        "{",
        "null",
        "[]",
        "{\"error\":null}",
        "{\"error\":{\"code\":\"x\"}}",
    ] {
        store
            .connection
            .execute(
                "INSERT OR REPLACE INTO requests VALUES('human','retry','body',?1,1)",
                [response],
            )
            .unwrap();
        assert_eq!(
            store.prior("human", "retry", "body").unwrap_err().code(),
            "database_error"
        );
        assert_eq!(
            store
                .prior("human", "retry", "different")
                .unwrap_err()
                .code(),
            "request_conflict"
        );
    }
    for response in [
        json!({"results":null}),
        json!({"results":[null]}),
        json!({"results":[],"unknown":true}),
        json!({"steering_target":{},"message_id":null}),
        json!({"message_id":"m","delivery_state":"future","recipient":"a","steering_target":{"execution_id":"e","turn_id":"t"}}),
    ] {
        assert_eq!(
            Queries::validate_replay(&response).unwrap_err().code(),
            "database_error"
        );
    }
}

#[test]
fn replay_protocol_mismatch_returns_original_request_id_without_reexecution() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    let original = json!({"message_id":"original","status":"accepted"});
    store.remember("human", "retry", "body", &original).unwrap();
    let changes = store.connection.total_changes();
    assert_eq!(
        store.prior("human", "retry", "body").unwrap(),
        Some(original.clone())
    );
    let mismatch = store
        .prior_protocol("human", "retry", "body", 2)
        .unwrap()
        .unwrap();
    assert_eq!(mismatch["error"]["code"], "replay_unavailable");
    assert_eq!(mismatch["request_id"], "retry");
    assert_eq!(store.connection.total_changes(), changes);
    assert_eq!(
        store.prior("human", "retry", "body").unwrap(),
        Some(original)
    );
    assert_eq!(
        store
            .prior_protocol("human", "retry", "changed", 2)
            .unwrap_err()
            .code(),
        "request_conflict"
    );
}

#[test]
fn single_agent_loads_build_the_agent_records_object_without_json_set() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    store.save(&agent("a", None)).unwrap();
    let view = |store: &Store| -> Value {
        let text: String = store
            .connection
            .query_row("SELECT data FROM agent_records WHERE id='a'", [], |row| {
                row.get(0)
            })
            .unwrap();
        serde_json::from_str(&text).unwrap()
    };
    let built = |store: &Store| -> Result<String> {
        let (data, columns) = store
            .connection
            .query_row(queries::AGENT_ROW, ["a"], agents::record_row)
            .unwrap();
        agents::record_text(&data, columns)
    };
    store
        .connection
        .pragma_update(None, "ignore_check_constraints", true)
        .unwrap();
    // Distinct values in every column catch a column order that disagrees with
    // the field list; then SQL types, flags and text that merely looks like JSON.
    for update in [
        "branch_hold=1, branch_gate='bg', ended_at=17, parent_id='p', auth='u', \
         auth_group='g', auth_checked_revision=3, waiting_reason='w', quota_wait_until=4, \
         lifetime='l', state='s', task_outcome='t', paused=1, expired=0, thread_id='th', \
         execution_id='ex', turn_id='tu', control='c', completion_pending='cp', result='r', \
         error='e', provider_id='pi', provider_pid=5, provider_spawn_before=6, \
         provider_spawn_after=7",
        "paused=1, expired=0",
        "paused=2, expired=NULL",
        "branch_hold=3, branch_gate='bound', ended_at=17",
        "result='{\"x\":1}', error='quote \" newline \n tab \t é'",
        "provider_pid=123, provider_spawn_before=4, provider_spawn_after=5",
        "parent_id=NULL, quota_wait_until=9223372036854775807",
        "data='{}'",
        "data='{ }'",
    ] {
        store
            .connection
            .execute(&format!("UPDATE agents SET {update} WHERE id='a'"), [])
            .unwrap();
        let text = built(&store).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            view(&store),
            "{update}"
        );
    }
    store
        .connection
        .pragma_update(None, "ignore_check_constraints", false)
        .unwrap();
    // A field left in data would be replaced by the view; here it is refused.
    store.save(&agent("b", None)).unwrap();
    store
        .connection
        .execute(
            "UPDATE agents SET data=json_set(data,'$.state','idle') WHERE id='b'",
            [],
        )
        .unwrap();
    assert_eq!(store.agent("b").unwrap_err().code(), "database_error");
    for update in ["provider_pid=1.5", "error=x'00'", "data='[]'"] {
        store.save(&agent("b", None)).unwrap();
        store
            .connection
            .execute(&format!("UPDATE agents SET {update} WHERE id='b'"), [])
            .unwrap();
        assert_eq!(
            store.agent("b").unwrap_err().code(),
            "database_error",
            "{update}"
        );
    }
    store.save(&agent("b", None)).unwrap();
    assert_eq!(
        api::value(store.agent("b").unwrap()),
        api::value(agent("b", None))
    );
}
