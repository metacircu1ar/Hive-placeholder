//! Recovery regressions.
use super::*;

#[test]
fn claude_session_options_relaunch_and_reset_without_stale_settings() {
    let mut f = Harness::builder(Provider::Claude)
        .scenario(json!({"provider_version":"2.1.287"}))
        .build();
    f.start();
    let id = super::super::claude::spawn(&f, "hold");
    let first = f.wait(&id, |a| a["state"] == "running");
    f.ok(json!({"op":"set_options","target":id,"options":{
        "claude.auto_compact_window":200_000,
        "claude.max_output_tokens":50000,
        "claude.max_mcp_output_tokens":4000
    }}));
    f.ok(json!({"op":"send","target":id,"body":"hold"}));
    f.ok(json!({"op":"interrupt","target":id}));
    let second = f.wait(&id, |a| {
        a["state"] == "running" && a["execution_id"] != first["execution_id"]
    });
    f.ok(json!({"op":"set_options","target":id,"options":{
        "claude.auto_compact_window":null,
        "claude.max_output_tokens":null,
        "claude.max_mcp_output_tokens":null
    }}));
    f.ok(json!({"op":"send","target":id,"body":"hold"}));
    f.ok(json!({"op":"interrupt","target":id}));
    f.wait(&id, |a| {
        a["state"] == "running" && a["execution_id"] != second["execution_id"]
    });

    let launches: Vec<Value> =
        fs::read_to_string(super::super::claude::home(&f).join("mock-launches.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    assert_eq!(launches.len(), 3, "{launches:?}");
    assert_eq!(launches[1]["settings"]["autoCompactWindow"], 200_000);
    assert_eq!(
        launches[1]["environment"]["CLAUDE_CODE_MAX_OUTPUT_TOKENS"],
        "50000"
    );
    assert_eq!(launches[1]["environment"]["MAX_MCP_OUTPUT_TOKENS"], "4000");
    assert!(launches[2]["settings"].get("autoCompactWindow").is_none());
    assert_eq!(
        launches[2]["environment"]["CLAUDE_CODE_MAX_OUTPUT_TOKENS"],
        Value::Null
    );
    assert_eq!(
        launches[2]["environment"]["MAX_MCP_OUTPUT_TOKENS"],
        Value::Null
    );
    f.shutdown();
}
