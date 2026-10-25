use super::*;

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// The login's usage report once its first row holds `windows` windows.
/// Rate-limit notices are recorded apart from the agent's state, so an idle
/// agent does not prove its notices have been read yet.
fn usage_for(f: &Harness, auth: &Value, windows: usize) -> Value {
    support::wait::wait_until(
        support::wait::TEST_TIMEOUT,
        support::wait::POLL_INTERVAL,
        "the login's usage report",
        || {
            let usage = f.ok(json!({"op":"usage","auth":auth}));
            let shown = usage["usage"][0]["windows"].as_array().map_or(0, Vec::len);
            (shown == windows).then_some(usage)
        },
    )
}

fn auth_usage(f: &Harness, name: Option<&str>) -> std::process::Output {
    support::command(env!("CARGO_BIN_EXE_hivectl"))
        .arg("--state-dir")
        .arg(&f.root)
        .args(["auth", "usage"])
        .args(name)
        .output()
        .unwrap()
}

#[test]
fn codex_usage_merges_the_start_read_with_turn_updates() {
    let now = now();
    let mut f = Harness::builder(Provider::Codex)
        .scenario(json!({
            // Every process start reads these; the turn's sparse update adds the
            // week, so the merge holds whichever process reports last.
            "rate_limits_read": {
                "limitId": "codex",
                "primary": {"usedPercent": 12, "windowDurationMins": 300, "resetsAt": now + 3600},
                "credits": {"hasCredits": true, "unlimited": false, "balance": "120"}
            },
            "rate_limits_update": {
                "secondary": {"usedPercent": 30, "windowDurationMins": 10080, "resetsAt": now + 259_200},
                "credits": null
            }
        }))
        .build();
    f.start();
    let id = f.spawn("reply: hi");
    let agent = f.settled(&id);
    let auth = agent["auth"].as_str().unwrap().to_owned();
    // The start read holds the 5-hour window; the turn's update adds the week.
    let usage = usage_for(&f, &json!(auth), 2);
    let rows = usage["usage"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{usage}");
    let row = &rows[0];
    assert_eq!(
        (&row["auth"], &row["provider"]),
        (&json!(auth), &json!("codex"))
    );
    assert_eq!(row["generation"], agent["auth_generation"], "{row}");
    let windows: Vec<(&str, f64)> = row["windows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| {
            (
                w["name"].as_str().unwrap(),
                w["used_percent"].as_f64().unwrap(),
            )
        })
        .collect();
    assert_eq!(windows, [("five_hour", 12.0), ("seven_day", 30.0)], "{row}");
    assert_eq!(row["windows"][1]["resets_at"], now + 259_200);
    assert_eq!(
        row["credits"],
        json!({"has_credits": true, "unlimited": false, "balance": "120"})
    );
    assert!(row.get("extra_usage").is_none());
    // An unknown login is refused; batches never reach the report.
    f.expect_err(json!({"op":"usage","auth":"other"}), "unknown_profile");
    assert_eq!(
        f.ok(json!({"op":"usage"}))["usage"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    f.expect_err(json!({"op":"usage","auth":7}), "invalid_value");
    let batch = f.call(json!({"op":"batch","operations":[{"op":"usage","request_id":"u"}]}));
    assert_eq!(batch["error"]["field"], "/operations/0/op", "{batch}");
    // The CLI prints the same reply; the daemon checks the profile name.
    for name in [Some(auth.as_str()), None] {
        let output = auth_usage(&f, name);
        assert!(output.status.success(), "{output:?}");
        let printed: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            printed["usage"][0]["credits"]["balance"], "120",
            "{printed}"
        );
    }
    let output = auth_usage(&f, Some("not-configured"));
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let refused: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(refused["error"]["code"], "unknown_profile", "{refused}");
    f.shutdown();
}

#[test]
fn claude_usage_reports_unified_windows_and_extra_usage() {
    let now = now();
    let mut f = Harness::builder(Provider::Claude).build();
    f.set_scenario(
        Provider::Claude,
        json!({"usage_info": {
            "status": "allowed",
            "resetsAt": now + 3600,
            "rateLimitType": "five_hour",
            "overageStatus": "allowed",
            "isUsingOverage": false,
            "unifiedWindows": {
                "five_hour": {"utilization": 0.52, "resetsAt": now + 3600},
                "seven_day": {"utilization": 0.44, "resetsAt": now + 400_000}
            }
        }}),
    );
    f.start();
    let id = claude::spawn(&f, "reply: hi");
    let agent = f.settled(&id);
    let usage = usage_for(&f, &agent["auth"], 2);
    let row = &usage["usage"][0];
    assert_eq!(row["provider"], "claude", "{usage}");
    assert_eq!(
        row["windows"],
        json!([
            {"name": "five_hour", "used_percent": 52.0, "resets_at": now + 3600},
            {"name": "seven_day", "used_percent": 44.0, "resets_at": now + 400_000}
        ])
    );
    assert_eq!(
        row["extra_usage"],
        json!({"status": "allowed", "in_use": false})
    );
    assert!(row.get("credits").is_none());
    f.shutdown();
}
