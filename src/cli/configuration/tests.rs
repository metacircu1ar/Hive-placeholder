use super::*;
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
};

#[test]
fn config_check_reports_groups_without_runtime_state_or_provider_calls() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("config.yaml");
    fs::write(&path, "auth_profiles: {a: {provider: codex, method: subscription}}\nauth_groups: {g: {mode: balanced, members: [a]}}\n").unwrap();
    for fix in [false, true] {
        let result = run(&path, &Command::Check { fix }).unwrap();
        let groups = &result.value["auth_groups"];
        assert_eq!(groups["g"]["mode"], "balanced");
        assert_eq!(groups["g"]["members"][0]["weight"], 1);
        assert_eq!(
            groups["g"]["members"][0]["bound_agents"],
            serde_json::Value::Null
        );
        assert_eq!(groups["g"]["load_available"], false);
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
}

#[test]
fn noop_fix_preserves_bytes_inode_mode_and_creates_no_backup() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("config.yaml");
    let text = "# Retain formatting\nauth_profiles: {}\n";
    fs::write(&path, text).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let before = fs::metadata(&path).unwrap();
    let result = run(&path, &Command::Check { fix: true }).unwrap();
    assert_eq!(result.value["changed"], false);
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
    let after = fs::metadata(&path).unwrap();
    assert_eq!(before.ino(), after.ino());
    assert_eq!(before.mode(), after.mode());
    assert_eq!(before.mtime_nsec(), after.mtime_nsec());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn fixes_and_setup_publication_resolve_native_configs_beside_the_target() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("configuration");
    fs::create_dir(&directory).unwrap();
    fs::write(
        directory.join("native.toml"),
        "tool_output_token_limit = 1234\n",
    )
    .unwrap();
    let target = directory.join("hive.yaml");
    let text = "auth_profiles: {main: {provider: codex, method: subscription}}\nroles: {worker: {auth: main, model: luna, effort: low, config: native.toml}}\n";
    fs::write(&target, text).unwrap();
    let link = root.path().join("config.yaml");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    run(&link, &Command::Check { fix: false }).unwrap();
    assert_eq!(
        run(&link, &Command::Check { fix: true }).unwrap().value["changed"],
        false
    );
    let snapshot = super::super::setup::publication::Snapshot::inspect(&link).unwrap();
    snapshot.publish(&format!("# Updated\n{text}")).unwrap();
    let parsed = Config::load(&link).unwrap();
    assert_eq!(
        parsed.roles["worker"].config.as_deref(),
        Some(
            directory
                .canonicalize()
                .unwrap()
                .join("native.toml")
                .as_path()
        )
    );
    assert!(
        fs::read_to_string(&target)
            .unwrap()
            .contains("config: native.toml")
    );
}
#[test]
fn known_fix_backs_up_original_and_preserves_configuration_symlink() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target.yaml");
    let link = root.path().join("config.yaml");
    let text = "# Original comment\nauth_profiles: {}\nruntime: {old_max_loaded: 128}\n";
    fs::write(&target, text).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let entries = [deprecations::Entry {
        path: &["runtime", "old_max_loaded"],
        replacement: Some("max_loaded"),
        since: "0.1.0",
        remove_in_major: 1,
    }];
    let result = fix(&link, &entries).unwrap();
    assert_eq!(result.value["changed"], true);
    assert_eq!(fs::read_link(&link).unwrap(), target);
    let backup = result.value["backup"].as_str().unwrap();
    assert_eq!(fs::read_to_string(backup).unwrap(), text);
    assert_eq!(fs::metadata(backup).unwrap().mode() & 0o777, 0o600);
    assert_eq!(fs::metadata(&target).unwrap().mode() & 0o777, 0o600);
    assert_eq!(Config::load(&link).unwrap().runtime.max_loaded, 128);
}
#[test]
fn failed_fix_or_missing_input_has_no_publication_side_effects() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("config.yaml");
    assert!(run(&path, &Command::Check { fix: true }).is_err());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    let text = "auth_profiles: {}\nruntime: {old_max_loaded: 128, max_loaded: 64}\n";
    fs::write(&path, text).unwrap();
    let entries = [deprecations::Entry {
        path: &["runtime", "old_max_loaded"],
        replacement: Some("max_loaded"),
        since: "0.1.0",
        remove_in_major: 1,
    }];
    assert!(fix(&path, &entries).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}
