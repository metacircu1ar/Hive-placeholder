use super::*;
use std::path::Path;

fn correct(text: &str, entries: &[Entry]) -> crate::error::Result<Correction> {
    super::correct(text, entries, Path::new("hive.yaml"))
}
const RENAMES: &[Entry] = &[Entry {
    path: &["roles", "*", "old_model"],
    replacement: Some("model"),
    since: "0.1.0",
    remove_in_major: 1,
}];
const INPUT: &str = "auth_profiles: {main: {provider: codex, method: subscription}}\nroles:\n  a: {auth: main, old_model: model-a}\n  b: {auth: main, old_model: model-b}\n";

#[test]
fn empty_current_table_does_not_admit_old_keys_or_reformat_valid_input() {
    assert!(TABLE.is_empty());
    assert!(correct(INPUT, TABLE).is_err());
    let current = "# Keep this comment\nauth_profiles: {}\nroles: {}\n";
    let corrected = correct(current, TABLE).unwrap();
    assert_eq!(corrected.text, current);
    assert!(corrected.applied.is_empty());
    assert!(corrected.remaining.is_empty());
}
#[test]
fn compiled_renames_preserve_values_and_never_report_user_names_or_values() {
    let notices = inspect(&super::super::Config::yaml(INPUT).unwrap(), RENAMES);
    assert_eq!(notices[0].key, "roles.*.old_model");
    assert_eq!(notices[0].occurrences, 2);
    let corrected = correct(INPUT, RENAMES).unwrap();
    assert_eq!(corrected.applied, notices);
    assert!(corrected.remaining.is_empty());
    let parsed = super::super::Config::parse(&corrected.text).unwrap();
    assert_eq!(parsed.roles["a"].model.as_deref(), Some("model-a"));
    assert_eq!(parsed.roles["b"].model.as_deref(), Some("model-b"));
    let metadata = serde_json::to_string(&notices).unwrap();
    assert!(!metadata.contains("model-a"));
    assert!(!metadata.contains("model-b"));
}
#[test]
fn fix_refuses_conflicts_invalid_final_config_and_yaml_indirection() {
    for text in [
        INPUT.replace("old_model: model-a", "old_model: model-a, model: existing"),
        INPUT.replace("old_model: model-a", "old_model: []"),
        INPUT.replace("old_model: model-a", "old_model: first, old_model: second"),
        INPUT.replace("old_model: model-a", "old_model: &alias model-a"),
        INPUT.replace("old_model: model-a", "old_model: !tag model-a"),
    ] {
        assert!(correct(&text, RENAMES).is_err());
    }
}
#[test]
fn manual_deprecations_remain_visible_without_guessing_a_conversion() {
    let entries = [Entry {
        path: &["runtime", "max_loaded"],
        replacement: None,
        since: "0.1.0",
        remove_in_major: 1,
    }];
    let input = "auth_profiles: {}\nruntime: {max_loaded: 128}\n";
    let corrected = correct(input, &entries).unwrap();
    assert_eq!(corrected.text, input);
    assert!(!corrected.remaining[0].fixable);
    assert!(corrected.applied.is_empty());
}
