use super::*;
use serde_json::json;

const NOW: u64 = 1_800_000_000;

#[test]
fn codex_and_claude_reports_read_as_one_line() {
    let reports = json!([
        {
            "auth": "codex-main", "provider": "codex", "observed_at": NOW - 180,
            "windows": [
                {"name": "five_hour", "used_percent": 12.0, "resets_at": NOW + 2 * 3600 + 38 * 60, "minutes": 300},
                {"name": "seven_day", "used_percent": 30.4, "resets_at": NOW + 5 * 86_400 + 18 * 3600, "minutes": 10080}
            ],
            "credits": {"has_credits": true, "unlimited": false, "balance": "120"}
        },
        {
            "auth": "claude-main", "provider": "claude", "observed_at": NOW - 30,
            "windows": [
                {"name": "five_hour", "used_percent": 52.0, "resets_at": NOW + 70 * 60},
                {"name": "seven_day", "used_percent": 44.0, "resets_at": NOW - 10},
                {"name": "seven_day_opus", "used_percent": 7.0}
            ],
            "extra_usage": {"status": "allowed", "in_use": false}
        }
    ]);
    assert_eq!(
        summary("codex-main", None, &reports, NOW),
        "codex-main: 5h 12% used, resets in 2h 38m · weekly 30% used, resets in 5d 18h · credits 120.00 · reported 3 min ago"
    );
    assert_eq!(
        summary("claude-main", None, &reports, NOW),
        "claude-main: 5h 52% used, resets in 1h 10m · weekly reset since this report · weekly Opus 7% used · extra usage available · reported just now"
    );
    assert_eq!(
        summary("codex-spare", None, &reports, NOW),
        "codex-spare: no usage reported yet; it is reported during this login's turns"
    );
    assert_eq!(
        summary("codex-main", None, &Value::Null, NOW),
        "codex-main: no usage reported yet; it is reported during this login's turns"
    );
}

#[test]
fn the_agents_generation_is_preferred_over_the_newest_report() {
    let reports = json!([
        {"auth": "main", "generation": "new", "observed_at": NOW,
         "windows": [{"name": "five_hour", "used_percent": 5.0}]},
        {"auth": "main", "generation": "old", "observed_at": NOW - 60,
         "windows": [{"name": "five_hour", "used_percent": 90.0}]}
    ]);
    assert_eq!(
        summary("main", Some("old"), &reports, NOW),
        "main: 5h 90% used · reported 1 min ago"
    );
    assert_eq!(
        summary("main", Some("gone"), &reports, NOW),
        "main: 5h 5% used · reported just now"
    );
}

#[test]
fn labels_credits_extra_usage_and_times_cover_every_variant() {
    assert_eq!(label("primary", Some(90)), "90m");
    assert_eq!(label("primary", Some(120)), "2h");
    assert_eq!(label("secondary", Some(2880)), "2d");
    assert_eq!(label("secondary", None), "secondary");
    assert_eq!(label("seven_day_oauth_apps", None), "weekly Oauth apps");
    assert_eq!(label("overage_window", None), "overage window");
    assert_eq!(
        credits_text(&views::read(
            &json!({"has_credits": true, "unlimited": true})
        )),
        "credits unlimited"
    );
    assert_eq!(
        credits_text(&views::read(
            &json!({"has_credits": true, "unlimited": false})
        )),
        "credits available"
    );
    assert_eq!(
        credits_text(&views::read(
            &json!({"has_credits": true, "unlimited": false, "balance": "12497.3847162500"})
        )),
        "credits 12497.38"
    );
    assert_eq!(
        credits_text(&views::read(
            &json!({"has_credits": true, "unlimited": false, "balance": "1,200"})
        )),
        "credits 1,200"
    );
    assert_eq!(
        credits_text(&views::read(
            &json!({"has_credits": false, "unlimited": false})
        )),
        "no credits"
    );
    assert_eq!(
        extra_text(&views::read(
            &json!({"status": "rejected", "in_use": false})
        )),
        "extra usage unavailable"
    );
    assert_eq!(
        extra_text(&views::read(
            &json!({"status": "allowed_warning", "in_use": true})
        )),
        "extra usage in use"
    );
    assert_eq!(span(59), "under a minute");
    assert_eq!(span(60), "1m");
    assert_eq!(span(3600), "1h 0m");
    assert_eq!(span(86_400 + 3599), "1d 0h");
    assert_eq!(ago(59), "just now");
    assert_eq!(ago(3599), "59 min ago");
    assert_eq!(ago(7200), "2h ago");
    assert_eq!(ago(3 * 86_400), "3d ago");
}
