//! Transport regressions.
use super::*;

#[test]
fn batch_inner_and_outer_replays_survive_caller_termination() {
    let temp = crate::runtime::test_directory();
    let mut core = core(temp.path());
    core.store().save(&agent("caller")).unwrap();
    core.store().save(&agent("recipient")).unwrap();
    let send =
        json!({"op":"send","target":"recipient","body":"work","request_id":"inner","extra":true});
    let batch = json!({
        "op": "batch",
        "operations": [
            send,
            {
                "op": "terminate",
                "target": "caller",
                "request_id": "cancel"
            },
            send
        ]
    });
    let accepted = call(&mut core, "caller", "batch", batch.clone()).unwrap();
    assert_eq!(accepted["results"].as_array().unwrap().len(), 3);
    assert_eq!(accepted["results"][0], accepted["results"][2]);
    assert_eq!(
        core.store().agent("caller").unwrap().task_outcome,
        TaskOutcome::Cancelled
    );
    assert_eq!(call(&mut core, "caller", "batch", batch).unwrap(), accepted);
    assert_eq!(
        core.store().messages("recipient", None, 200).unwrap()["messages"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        core.store()
            .prior(
                "caller",
                "inner",
                &json!({"op":"send","target":"recipient","body":"work","extra":true}).to_string()
            )
            .unwrap()
            .unwrap(),
        accepted["results"][0]
    );
}

#[test]
fn internal_restoration_replay_uses_a_distinct_caller_namespace() {
    use crate::runtime::{core::preflight::Facts, hub::Caller};
    let tmp = crate::runtime::test_directory();
    let mut core = core(tmp.path());
    let request = RequestEnvelope::new(json!({"op":"login_restored", "auth":"main"}));
    let body = request.fingerprint();
    core.store()
        .remember(
            Caller::Human.name(),
            "same-id",
            &body,
            &json!({"owner":"human"}),
        )
        .unwrap();
    assert!(
        core.replay(&Caller::System, "same-id", &request)
            .unwrap()
            .is_none()
    );
    core.store()
        .remember(
            Caller::System.name(),
            "same-id",
            &body,
            &json!({"owner":"system"}),
        )
        .unwrap();
    for (caller, owner) in [(Caller::Human, "human"), (Caller::System, "system")] {
        assert_eq!(
            core.dispatch(&caller, "same-id", &request, &Facts::default())
                .unwrap(),
            json!({"owner":owner})
        );
        let changed = RequestEnvelope::new(json!({"op":"login_restored", "auth":"other"}));
        assert!(matches!(
            core.replay(&caller, "same-id", &changed),
            Err(Error::RequestConflict { .. })
        ));
    }
    assert!(
        core.replay(&Caller::test_agent("agent"), "same-id", &request)
            .unwrap()
            .is_none()
    );
}

#[test]
fn system_caller_has_only_internal_restoration_permission() {
    use crate::runtime::{core::preflight::Facts, hub::Caller};
    let tmp = crate::runtime::test_directory();
    let mut core = core(tmp.path());
    for raw in [
        json!({"op":"ping"}),
        json!({"op":"shutdown"}),
        json!({"op":"set_auth", "target":"agent", "auth":"main"}),
        json!({"op":"purge", "target":"agent"}),
        json!({"op":"approvals"}),
        json!({"op":"gc", "before":1}),
    ] {
        let request = RequestEnvelope::new(raw);
        assert!(matches!(
            core.dispatch(&Caller::System, "internal", &request, &Facts::default()),
            Err(Error::UnsupportedOperation { .. })
        ));
    }
    let restore = Request::LoginRestored(AuthArgs {
        auth: "main".to_owned().into(),
    });
    assert!(restore.caller_policy(&Caller::System).is_ok());
    assert!(restore.caller_policy(&Caller::Human).is_ok());
    assert!(restore.caller_policy(&Caller::test_agent("agent")).is_err());
    assert_eq!(value(Caller::Human), json!("human"));
    assert_eq!(value(Caller::test_agent("agent")), json!("agent"));
}
