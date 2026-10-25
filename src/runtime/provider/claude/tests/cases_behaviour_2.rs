//! Behaviour regressions.
#[cfg(feature = "comb")]
use super::*;

#[test]
#[cfg(feature = "comb")]
fn comb_and_extra_mcp_names_obey_both_permission_paths_and_canary() {
    for (tool, allowed) in [
        ("mcp__hive__comb_get", true),
        ("mcp__hive__comb_promote", false),
        ("mcp__hive__hive_send", true),
        ("mcp__hive__hive_invented", false),
        ("mcp__docs__lookup", true),
        ("mcp__docs_extra__lookup", false),
        ("mcp__unconfigured__lookup", false),
    ] {
        let mut f = Fixture::with_extensions(None, Uuid::new_v4().to_string(), |config| {
            config.comb_reviewer = Some(false);
            config.mcp_servers.insert(
                "docs".into(),
                crate::config::McpServer {
                    command: "fixture".into(),
                    args: vec![],
                    env: std::collections::BTreeMap::default(),
                },
            );
        });
        f.turn();
        assert_eq!(
            f.hook(tool, json!({}))["permissionDecision"],
            if allowed { "allow" } else { "deny" },
            "{tool}"
        );
        let reply = f.codec.receive(json!({"type":"control_request","request_id":"permission","request":{"subtype":"can_use_tool","tool_name":tool,"input":{}}})).unwrap();
        assert_eq!(
            reply[0]["response"]["response"]["behavior"],
            if allowed { "allow" } else { "deny" },
            "{tool}"
        );
    }
    let mut f = Fixture::with_extensions(
        Some(SteeringPermit::Canary),
        Uuid::new_v4().to_string(),
        |config| config.comb_reviewer = Some(true),
    );
    f.turn();
    assert_eq!(
        f.hook("mcp__hive__comb_get", json!({}))["permissionDecision"],
        "deny"
    );
}
