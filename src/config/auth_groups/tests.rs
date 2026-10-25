use super::*;

const PROFILES: &str = "auth_profiles:\n  a: {provider: codex, method: subscription}\n  b: {provider: codex, method: subscription}\n  c: {provider: claude, method: subscription}\n  api: {provider: codex, method: api_key, api_key_env: KEY}\n";

#[test]
fn auth_groups_normalize_weights_and_keep_subscription_and_paid_order() {
    let config = Config::parse(&format!("{PROFILES}auth_groups:\n  pool:\n    mode: balanced\n    members: [a, {{profile: b, weight: 4}}]\n    paid_tail: [api]\n")).unwrap();
    let group = &config.auth_groups["pool"];
    assert_eq!(group.mode, AuthMode::Balanced);
    assert_eq!(
        group
            .members
            .iter()
            .map(AuthMember::weight)
            .collect::<Vec<_>>(),
        [1, 4]
    );
    assert_eq!(config.auth_members("pool").unwrap(), ["a", "b", "api"]);
    assert_eq!(config.auth_provider("pool").unwrap(), Provider::Codex);
    assert_eq!(config.auth_members("a").unwrap(), ["a"]);
    let summary = config.auth_group_summary();
    assert_eq!(summary["pool"]["members"][0]["weight"], 1);
    assert_eq!(summary["pool"]["load_available"], false);
    assert_eq!(config.auth_group_members()["pool"], ["a", "b", "api"]);
    for mode in ["ordered", "balanced"] {
        let config = Config::parse(&format!(
            "{PROFILES}auth_groups: {{g: {{mode: {mode}, members: [{{profile: a}}]}}}}"
        ))
        .unwrap();
        assert_eq!(config.auth_groups["g"].members[0].weight(), 1);
        // Only balanced groups report weights.
        assert_eq!(
            config.auth_group_summary()["g"]["members"][0]
                .get("weight")
                .is_some(),
            mode == "balanced"
        );
    }
}

#[test]
fn auth_groups_refuse_old_shapes_billing_errors_weights_and_unknown_fields() {
    for group in [
        "[a, b]",
        "{}",
        "{mode: ordered}",
        "{members: [a]}",
        "{mode: random, members: [a]}",
        "{mode: balanced, members: []}",
        "{mode: balanced, members: [api]}",
        "{mode: ordered, members: [a], paid_tail: [b]}",
        "{mode: balanced, members: [a, a]}",
        "{mode: balanced, members: [a, {profile: a}]}",
        "{mode: balanced, members: [a, c]}",
        "{mode: balanced, members: [missing]}",
        "{mode: balanced, members: [a], paid_tail: [missing]}",
        "{mode: balanced, members: [a], paid_tail: [api, api]}",
        "{mode: balanced, members: [{profile: a, weight: 0}]}",
        "{mode: balanced, members: [{profile: a, weight: 1001}]}",
        "{mode: balanced, members: [{profile: a, weight: -1}]}",
        "{mode: balanced, members: [{profile: a, weight: 1.5}]}",
        "{mode: balanced, members: [{profile: a, weight: '4'}]}",
        "{mode: balanced, members: [{profile: a, weight: null}]}",
        "{mode: balanced, members: [{profile: a, weight: true}]}",
        "{mode: ordered, members: [{profile: a, weight: 1}]}",
        "{mode: ordered, members: [a], unknown: true}",
        "{mode: balanced, members: [{profile: a, typo: 3}]}",
    ] {
        assert!(
            Config::parse(&format!("{PROFILES}auth_groups: {{g: {group}}}")).is_err(),
            "{group}"
        );
    }
    assert!(
        Config::parse(&format!(
            "{PROFILES}auth_groups: {{a: {{mode: ordered, members: [a]}}}}"
        ))
        .is_err()
    );
    let maximum = Config::parse(&format!(
        "{PROFILES}auth_groups: {{g: {{mode: balanced, members: [{{profile: a, weight: 1000}}]}}}}"
    ))
    .unwrap();
    assert_eq!(maximum.auth_groups["g"].members[0].weight(), 1000);
}
