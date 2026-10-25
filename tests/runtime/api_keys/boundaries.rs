use super::*;

#[test]
fn rotation_does_not_interrupt_an_active_old_key_turn() {
    let (mut f, _) = runtime_fixture(Provider::Codex);
    f.start();
    let old = f.spawn("hold");
    let running = f.wait(&old, |a| a["state"] == "running");
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .write(f.tmp.path().join("api-key"), ROTATED.as_bytes())
        .unwrap();
    let new = f.spawn("reply: fresh key");
    let newer = startup_finished(&f, &new);
    assert_ne!(running["provider_id"], newer["provider_id"]);
    assert_eq!(newer["execution_key"]["revision"], 2);
    let retained = f.status(&old);
    assert_eq!(retained["state"], "running");
    assert_eq!(retained["turn_id"], running["turn_id"]);
    assert_eq!(retained["execution_key"]["revision"], 1);
    f.ok(json!({"op":"interrupt","target":old}));
    f.wait(&old, |a| a["turn_id"].is_null());
    let sent = f.ok(json!({"op":"send","target":old,"body":"reply: same old process"}));
    let later = finished(&f, &old, &sent);
    assert_eq!(later["provider_id"], running["provider_id"]);
    assert_eq!(later["execution_key"]["revision"], 1);
    f.shutdown();
    tree_private(&f.root);
}

#[test]
fn api_structured_limits_require_a_known_reset_and_spending_never_retries() {
    for (error, reset, expected, reason) in [
        (
            "usageLimitExceeded",
            json!(hive::state::now() + 3600),
            "limited",
            "usage_limit",
        ),
        (
            "usageLimitExceeded",
            Value::Null,
            "unavailable",
            "usage_limit",
        ),
        ("billingError", Value::Null, "unavailable", "spend_cap"),
    ] {
        let (mut f, home) = runtime_fixture(Provider::Codex);
        Harness::write_scenario(
            &home,
            json!({"api_key":true,"quota_once":true,
            "quota_error":error,"quota_until":reset}),
        );
        f.start();
        let id = f.spawn("reply: limited");
        f.wait(&id, |a| a["waiting_reason"] == "quota");
        let status = f.ok(json!({"op":"auth_availability","auth":"api"}));
        let value = &status["availability"][0];
        assert_eq!(value["state"], expected, "{status}");
        assert_eq!(value["reason"], reason, "{status}");
        assert_eq!(value["until"], reset);
        assert!(value["retry_at"].is_null());
        f.shutdown();
        tree_private(&f.root);
    }
}

#[test]
fn recovered_confirmed_subscription_replaces_api_at_the_next_boundary() {
    for compatible in [true, false] {
        let (mut f, _) = runtime_fixture(Provider::Codex);
        f.edit_config(|config| {
            config["auth_groups"]["mixed"] =
                json!({"mode":"ordered","members":["main"],"paid_tail":["api"]});
            config["roles"]["implementor"]["auth"] = json!("mixed");
        });
        f.set_scenario(
            Provider::Codex,
            json!({"quota_once":true,"quota_seconds":2,"catalog_models":["gpt-5.6-terra"]}),
        );
        f.start();
        let id = f.spawn("reply: exhausted plan");
        f.wait(&id, |a| a["auth"] == "api" && a["state"] == "idle");
        if !compatible {
            f.ok(json!({"op":"set_options","target":id,"options":{"model":"gpt-5.6-sol","effort":"medium"}}));
        }
        // A separate explicit subscription agent confirms recovery with actual work.
        let recovery = f.ok(json!({"op":"spawn","role":"implementor","auth":"main",
            "cwd":f.tmp.path(),"body":"reply: recovered plan"}));
        startup_finished(&f, recovery["agent_id"].as_str().unwrap());
        let status = f.ok(json!({"op":"auth_availability","auth":"main"}));
        assert!(
            status["availability"][0]["confirmed_at"].is_number(),
            "{status}"
        );
        let sent = f.ok(json!({"op":"send","target":id,"body":"reply: back to plan"}));
        let returned = finished(&f, &id, &sent);
        assert_eq!(returned["auth"], if compatible { "main" } else { "api" });
        assert_eq!(
            returned["billing"],
            if compatible {
                "subscription"
            } else {
                "api_key"
            }
        );
        f.shutdown();
        tree_private(&f.root);
    }
}
