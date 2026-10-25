use super::*;
use control_inputs::assert_input;
use pagination::observe;

fn ids(c: &Core, args: Value) -> Vec<String> {
    observe(c, args).unwrap()["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn auth_filters_match_current_profiles_and_groups_after_config_removal() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    let mut a = agent("bound");
    a.auth_group = Some("plans".into());
    a.auth_switch_pending = Some(crate::runtime::auth_fallback::SwitchIntent {
        target: "next".into(),
        reason: crate::runtime::auth_fallback::AuthReason::Manual,
        source_generation: a.auth_generation,
        source_execution: None,
    });
    c.store().save(&a).unwrap();
    c.store().save(&agent("direct")).unwrap();
    for name in ["main", "next", "unused"] {
        c.config_mut()
            .auth_profiles
            .insert(name.into(), a.profile.clone());
    }
    c.config_mut().auth_groups.insert(
        "plans".into(),
        crate::config::AuthGroup::ordered(vec!["main".into()]),
    );
    c.config_mut().auth_groups.insert(
        "unused-group".into(),
        crate::config::AuthGroup::ordered(vec!["unused".into()]),
    );
    for name in ["next", "unused", "unused-group"] {
        assert!(ids(&c, json!({"op":"agents","auth":name})).is_empty());
    }
    for configured in [true, false] {
        if !configured {
            c.config_mut().auth_profiles.clear();
            c.config_mut().auth_groups.clear();
        }
        assert_eq!(ids(&c, json!({"op":"agents","auth":"plans"})), ["bound"]);
        assert_eq!(
            ids(&c, json!({"op":"agents","auth":"main"})),
            ["bound", "direct"]
        );
        assert!(ids(&c, json!({"op":"agents","auth":"plans","after":u64::MAX})).is_empty());
    }
    // A pending future login is not a retained binding after config removal.
    assert_input(
        observe(&c, json!({"op":"agents","auth":"next"})),
        "invalid_value",
        "/auth",
        "agents",
    );
}

#[test]
fn purged_parent_still_selects_surviving_children() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    let mut parent = agent("parent");
    parent.state = AgentState::Stopped;
    parent.task_outcome = TaskOutcome::Completed;
    c.store().save(&parent).unwrap();
    let mut child = agent("child");
    child.parent_id = Some(parent.id.clone());
    c.store().save(&child).unwrap();
    for purged in [false, true] {
        if purged {
            call(
                &mut c,
                "human",
                "purge",
                json!({"op":"purge","target":"parent"}),
            )
            .unwrap();
        }
        assert_eq!(ids(&c, json!({"op":"agents","parent":"parent"})), ["child"]);
        assert!(ids(&c, json!({"op":"agents","parent":"child"})).is_empty());
        assert!(
            ids(
                &c,
                json!({"op":"agents","parent":"parent","after":u64::MAX})
            )
            .is_empty()
        );
    }
    assert_eq!(
        c.store().agent("parent").unwrap_err().code(),
        "agent_purged"
    );
}

#[test]
fn unknown_filters_error_even_when_pagination_or_other_filters_exclude_rows() {
    let temp = crate::runtime::test_directory();
    let c = core(temp.path());
    c.store().save(&agent("known")).unwrap();
    for raw in [
        json!({"op":"agents","parent":"private-canary"}),
        json!({"op":"agents","parent":"private-canary","after":u64::MAX,"auth":"main"}),
    ] {
        assert_input(observe(&c, raw), "invalid_value", "/parent", "agents");
    }
    for raw in [
        json!({"op":"agents","auth":"private-canary"}),
        json!({"op":"agents","auth":"private-canary","parent":"known","after":u64::MAX}),
    ] {
        assert_input(observe(&c, raw), "invalid_value", "/auth", "agents");
    }
}

#[test]
fn list_filter_syntax_retains_page_and_type_error_order() {
    let temp = crate::runtime::test_directory();
    let c = core(temp.path());
    for (raw, code, field) in [
        (
            json!({"op":"agents","parent":"","auth":42}),
            "invalid_value",
            "/auth",
        ),
        (
            json!({"op":"agents","parent":"","auth":"bad name"}),
            "out_of_range",
            "/parent",
        ),
        (
            json!({"op":"agents","parent":"é".repeat(32769)}),
            "out_of_range",
            "/parent",
        ),
        (json!({"op":"agents","auth":"é"}), "invalid_value", "/auth"),
        (
            json!({"op":"agents","auth":"x".repeat(65)}),
            "invalid_value",
            "/auth",
        ),
        (
            json!({"op":"agents","parent":"unknown","limit":0}),
            "out_of_range",
            "/limit",
        ),
        (
            json!({"op":"agents","parent":"unknown","limit":0,"after":null}),
            "invalid_value",
            "/after",
        ),
    ] {
        assert_input(observe(&c, raw), code, field, "agents");
    }
}
