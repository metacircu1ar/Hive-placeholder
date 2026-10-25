use super::*;

provider_matrix!(
    lifetimes_terminate_active_bound_children_on_both_providers_and_survive_restart,
    lifetimes_terminate_active_bound_children_on_both_providers_and_survive_restart_case
);

fn lifetimes_terminate_active_bound_children_on_both_providers_and_survive_restart_case(
    provider: Provider,
) {
    let use_claude = provider == Provider::Claude;
    let mut f = if use_claude {
        claude::fixture()
    } else {
        Harness::new()
    };
    f.start();
    let parent = if use_claude {
        claude::spawn(&f, "spawn bound child")
    } else {
        f.spawn("spawn bound child")
    };
    f.wait(&parent, |a| {
        a["state"] == "idle"
            && f.mailbox(&parent)["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["sender"] == "human" && m["state"] == "completed")
    });
    let all = f.call(json!({"op":"agents","parent":parent}));
    let children = all["agents"].as_array().unwrap();
    assert_eq!(children.len(), 1, "{all}");
    let child = children[0]["id"].as_str().unwrap().to_owned();
    let running = f.wait(&child, |a| a["state"] == "running");
    assert_eq!(running["lifetime"], "with_parent");
    assert_eq!(running["parent_id"], parent);
    let queued = f.call(json!({"op":"send","target":child,"body":"queued input"}));
    let args = json!({"op":"terminate","target":parent});
    let accepted = f.call_id("end-parent", args.clone());
    assert!(accepted.get("error").is_none(), "{accepted}");
    let expired = f.wait(&child, |a| a["state"] == "stopped");
    assert_eq!(expired["expired"], true);
    assert_eq!(expired["task_outcome"], "open");
    let result = f.call(json!({"op":"message_result","message_id":queued["message_id"]}));
    assert_eq!(result["delivery_state"], "undeliverable");
    let events = f.read(&child);
    let entries = events["events"].as_array().unwrap();
    let expiry: Vec<_> = entries
        .iter()
        .filter(|e| e["type"] == "lifetime_expired")
        .collect();
    assert_eq!(expiry.len(), 1);
    assert_eq!(
        expiry[0]["data"],
        json!({"parent_id":parent,"reason":"parent_terminated"})
    );
    let stopped = entries
        .iter()
        .find(|e| e["type"] == "execution_stopped")
        .unwrap();
    assert!(
        entries.iter().any(|e| {
            matches!(
                e["type"].as_str(),
                Some("provider_shutdown" | "provider_session_released")
            ) && e["execution_id"] == stopped["execution_id"]
                && e["data"]["confirmed"] == true
                && e["data"]["quiescent"] == true
                && e["seq"].as_u64().unwrap() < stopped["seq"].as_u64().unwrap()
        }),
        "{events}"
    );
    f.wait(&parent, |a| a["state"] == "stopped");
    f.shutdown();
    f.start();
    assert_eq!(f.call_id("end-parent", args.clone()), accepted);
    assert_eq!(f.call(args)["error"]["code"], "agent_completed");
    assert_eq!(
        f.call(json!({"op":"resume","target":child}))["error"]["code"],
        "agent_completed"
    );
    f.shutdown();
    assert!(f.reports().iter().all(|r| r["confirmed"] == true));
}

#[test]
fn lifetimes_cli_spawn_overrides_role_default_and_rejects_bound_human_spawns() {
    let mut f = Harness::new();
    f.start();
    let body = f.tmp.path().join("body.txt");
    fs::write(&body, "idle").unwrap();
    for (role, lifetime, expected) in [
        ("reviewer", "independent", None),
        ("implementor", "with_parent", Some("invalid_value")),
        ("implementor", "persistent", Some("invalid_value")),
    ] {
        let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
            .arg("--state-dir")
            .arg(&f.root)
            .args(["spawn", role, "--cwd"])
            .arg(f.tmp.path())
            .arg("--body-file")
            .arg(&body)
            .args(["--lifetime", lifetime])
            .output()
            .unwrap();
        let result: Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&output.stderr)));
        if let Some(expected) = expected {
            assert_eq!(output.status.code(), Some(2));
            assert_eq!(result["error"]["field"], "--lifetime");
            assert_eq!(result["error"]["code"], expected, "{result}");
        } else {
            assert!(output.status.success(), "{result}");
            let id = result["agent_id"].as_str().unwrap();
            let a = f.wait(id, |a| a["state"] == "idle");
            assert_eq!(a["lifetime"], "independent");
            assert!(a["parent_id"].is_null());
        }
    }
    f.shutdown();
}

#[test]
fn accepted_spawn_replays_after_directory_removal_and_wire_parent_rule_is_preserved() {
    let mut f = Harness::new();
    f.start();
    let cwd = f.tmp.path().join("disposable-workspace");
    fs::create_dir(&cwd).unwrap();
    let request = json!({"op":"spawn","role":"implementor","cwd":cwd,"body":"idle"});
    let first = f.call_id("existing-cwd", request.clone());
    assert!(first.get("error").is_none(), "{first}");
    let id = first["agent_id"].as_str().unwrap();
    f.wait(id, |a| a["state"] == "idle");
    f.ok(json!({"op":"stop","target":id}));
    f.wait(id, |a| a["state"] == "stopped");
    fs::remove_dir(&cwd).unwrap();
    assert_eq!(f.call_id("existing-cwd", request), first);
    let rejected = f.call(json!({"op":"spawn","role":"implementor","cwd":f.tmp.path(),"body":"idle","lifetime":"with_parent"}));
    assert_eq!(rejected["error"]["code"], "parent_required");
    f.shutdown();
}
