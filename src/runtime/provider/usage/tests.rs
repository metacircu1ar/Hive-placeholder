use super::*;
use serde_json::json;

const NOW: u64 = 1_800_000_000;

fn generation(n: u128) -> uuid::Uuid {
    uuid::Uuid::from_u128(n)
}

fn names(report: &Report) -> Vec<(&str, f64, Option<u64>)> {
    report
        .windows
        .iter()
        .map(|w| (w.name.as_str(), w.used_percent, w.resets_at))
        .collect()
}

#[test]
fn codex_reads_merge_sparse_updates_and_keep_only_the_codex_limit() {
    let usages = Usages::default();
    usages.codex(
        "codex-main",
        generation(1),
        &json!({
            "limitId": "codex",
            "primary": {"usedPercent": 12, "windowDurationMins": 300, "resetsAt": NOW + 600},
            "secondary": {"usedPercent": 30, "windowDurationMins": 10080, "resetsAt": NOW + 86_400},
            "credits": {"hasCredits": true, "unlimited": false, "balance": "120.50"},
            "planType": "plus"
        }),
        NOW,
    );
    // A sparse update replaces only what it carries; nulls keep the old value.
    usages.codex(
        "codex-main",
        generation(1),
        &json!({"primary": {"usedPercent": 14, "windowDurationMins": 300, "resetsAt": NOW + 500},
                "secondary": null, "credits": null}),
        NOW + 60,
    );
    // Another bucket must not overwrite the account's limit.
    usages.codex(
        "codex-main",
        generation(1),
        &json!({"limitId": "codex_other", "primary": {"usedPercent": 99}}),
        NOW + 90,
    );
    let reports = usages.reports(None);
    assert_eq!(reports.len(), 1);
    let report = &reports[0];
    assert_eq!(report.provider, Provider::Codex);
    assert_eq!(report.observed_at, NOW + 60);
    assert_eq!(
        names(report),
        [
            ("five_hour", 14.0, Some(NOW + 500)),
            ("seven_day", 30.0, Some(NOW + 86_400))
        ]
    );
    assert_eq!(report.windows[0].minutes, Some(300));
    assert_eq!(
        report.credits,
        Some(CodexCredits {
            has_credits: true,
            unlimited: false,
            balance: Some(String::from("120.50"))
        })
    );
    assert_eq!(report.extra_usage, None);
    assert_eq!(
        serde_json::to_value(report).unwrap(),
        json!({
            "auth": "codex-main",
            "provider": "codex",
            "generation": generation(1).to_string(),
            "observed_at": NOW + 60,
            "windows": [
                {"name": "five_hour", "used_percent": 14.0, "resets_at": NOW + 500, "minutes": 300},
                {"name": "seven_day", "used_percent": 30.0, "resets_at": NOW + 86_400, "minutes": 10080}
            ],
            "credits": {"has_credits": true, "unlimited": false, "balance": "120.50"}
        })
    );
}

#[test]
fn codex_values_outside_their_bounds_are_dropped() {
    let usages = Usages::default();
    usages.codex(
        "codex-main",
        generation(1),
        &json!({
            "primary": {"usedPercent": 40, "windowDurationMins": 90, "resetsAt": NOW + 40 * 86_400},
            "secondary": {"usedPercent": -1},
            "credits": {"hasCredits": false, "unlimited": false, "balance": "12 USD"}
        }),
        NOW,
    );
    let report = &usages.reports(Some("codex-main"))[0];
    // An unknown duration keeps the slot name; an implausible reset is dropped.
    assert_eq!(names(report), [("primary", 40.0, None)]);
    assert_eq!(report.windows[0].minutes, Some(90));
    assert_eq!(report.credits.as_ref().unwrap().balance, None);
    // Nothing usable creates no snapshot.
    usages.codex(
        "codex-other",
        generation(1),
        &json!({"rateLimitReachedType": "rate_limit_reached", "primary": {"usedPercent": "x"}}),
        NOW,
    );
    usages.codex("codex-other", generation(1), &Value::Null, NOW);
    assert!(usages.reports(Some("codex-other")).is_empty());
    let huge = Value::from(f64::MAX);
    usages.codex(
        "codex-other",
        generation(1),
        &json!({"primary": {"usedPercent": huge}, "credits": {"hasCredits": true}}),
        NOW,
    );
    assert!(usages.reports(Some("codex-other")).is_empty());
}

#[test]
fn claude_events_report_unified_windows_and_extra_usage() {
    let usages = Usages::default();
    usages.claude(
        "claude-main",
        generation(2),
        &json!({
            "status": "allowed",
            "resetsAt": NOW + 3600,
            "rateLimitType": "five_hour",
            "overageStatus": "allowed",
            "isUsingOverage": false,
            "unifiedWindows": {
                "seven_day_fable": {"utilization": 0.07, "resetsAt": NOW + 500_000},
                "seven_day": {"utilization": 0.44, "resetsAt": NOW + 500_000},
                "five_hour": {"utilization": 0.52, "resetsAt": NOW + 3600},
                "Bad-Name": {"utilization": 0.1},
                "no_utilization": {"resetsAt": NOW}
            }
        }),
        NOW,
    );
    let report = &usages.reports(None)[0];
    assert_eq!(report.provider, Provider::Claude);
    assert_eq!(
        names(report),
        [
            ("five_hour", 52.0, Some(NOW + 3600)),
            ("seven_day", 44.0, Some(NOW + 500_000)),
            ("seven_day_fable", 7.0, Some(NOW + 500_000)),
        ]
    );
    assert_eq!(
        report.extra_usage,
        Some(ExtraUsage {
            status: ExtraStatus::Allowed,
            in_use: false
        })
    );
    // A later event without windows still updates extra usage and keeps them.
    usages.claude(
        "claude-main",
        generation(2),
        &json!({"overageStatus": "rejected", "isUsingOverage": true}),
        NOW + 5,
    );
    let report = &usages.reports(None)[0];
    assert_eq!(report.windows.len(), 3);
    assert_eq!(
        report.extra_usage,
        Some(ExtraUsage {
            status: ExtraStatus::Rejected,
            in_use: true
        })
    );
    assert_eq!(report.observed_at, NOW + 5);
    // Unknown overage states are not reported.
    usages.claude(
        "claude-two",
        generation(2),
        &json!({"overageStatus": "something_new"}),
        NOW,
    );
    assert!(usages.reports(Some("claude-two")).is_empty());
}

#[test]
fn generations_are_kept_apart_newest_first_and_windows_are_bounded() {
    let usages = Usages::default();
    let many: serde_json::Map<String, Value> = (0..12)
        .map(|i| (format!("w{i:02}"), json!({"utilization": 0.1})))
        .collect();
    usages.claude(
        "login",
        generation(1),
        &json!({"unifiedWindows": many}),
        NOW,
    );
    assert_eq!(usages.reports(None)[0].windows.len(), MAX_WINDOWS);
    // A repaired login's new generation reports beside the old one.
    usages.claude(
        "login",
        generation(2),
        &json!({"unifiedWindows": {"five_hour": {"utilization": 0.2}}}),
        NOW + 1,
    );
    usages.claude(
        "a-login",
        generation(3),
        &json!({"unifiedWindows": {"five_hour": {"utilization": 0.3}}}),
        NOW,
    );
    let order: Vec<(String, String)> = usages
        .reports(None)
        .into_iter()
        .map(|report| (report.auth, report.generation))
        .collect();
    assert_eq!(
        order,
        [
            (String::from("a-login"), generation(3).to_string()),
            (String::from("login"), generation(2).to_string()),
            (String::from("login"), generation(1).to_string()),
        ]
    );
    let newest = &usages.reports(Some("login"))[0];
    assert_eq!(names(newest), [("five_hour", 20.0, None)]);
    // Another provider under the same generation starts over.
    usages.codex(
        "login",
        generation(2),
        &json!({"primary": {"usedPercent": 1, "windowDurationMins": 300}}),
        NOW + 2,
    );
    let report = &usages.reports(Some("login"))[0];
    assert_eq!(report.provider, Provider::Codex);
    assert_eq!(names(report), [("five_hour", 1.0, None)]);
    assert_eq!(usages.reports(Some("login")).len(), 2);
}
