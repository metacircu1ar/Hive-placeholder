use super::*;
use serde_json::json;

#[test]
fn double_failure_keeps_primary_native_cause_and_logs_sanitized_cleanup_cause() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("cleanup.jsonl");
    let (backend, guard) = hive_log::start(&path, hive_log::Config::default()).unwrap();
    let parse = serde_json::from_str::<u64>("\"private-probe-output-canary\"").unwrap_err();
    let primary = Error::version_unrecognized("Version probe failed.").with_source(parse);
    let secondary = Error::state_io_from(std::io::Error::from_raw_os_error(libc::ENOSPC));
    let error = backend.scope(|| {
        crate::observability::settle::<()>(Err(primary), Err(secondary), "metadata_probe")
            .unwrap_err()
    });
    assert_eq!(error.code(), "version_unrecognized");
    assert!(
        std::error::Error::source(&error)
            .unwrap()
            .is::<serde_json::Error>()
    );
    assert_eq!(
        guard.finish(std::time::Duration::from_secs(1)).state,
        hive_log::Shutdown::Drained
    );
    let text = std::fs::read_to_string(path).unwrap();
    assert!(!text.contains("private-probe-output-canary"));
    assert_eq!(text.lines().count(), 1);
    let record: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(record["event"], "cleanup.secondary_failed");
    assert!(text.contains("json_data"));
    assert!(text.contains(&format!("\"errno\":{}", libc::ENOSPC)));
}

#[test]
fn recovery_rejects_incomplete_credential_attempt_instead_of_skipping_it() {
    use crate::{auth::Settings, config::Config, process::Cancellation, state::State};
    use std::{fs, time::Duration};
    let temporary = tempfile::tempdir().unwrap();
    let config =
        Config::parse("auth_profiles:\n  main:\n    provider: codex\n    method: subscription\n")
            .unwrap();
    let state = State::new(temporary.path().join("state")).unwrap();
    let binding = state
        .register("main", config.profile("main").unwrap())
        .unwrap();
    let index = state.root.join("index.json");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&index).unwrap()).unwrap();
    value["generations"][binding.id.to_string()]["credential_mutation"] =
        json!(uuid::Uuid::new_v4());
    // The attempt exists, but no writer can be identified. Structural parsing
    // and profile lookup succeed; the pending-attempt query must reject this.
    fs::write(&index, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(
        state
            .lookup("main", config.profile("main").unwrap())
            .unwrap()
            .is_some()
    );
    let auth = Auth {
        config,
        state,
        settings: Settings {
            launcher: crate::launch::test_cache().unwrap(),
            codex: "unused".into(),
            claude: "unused".into(),
            probe_timeout: Duration::from_secs(1),
            login_timeout: Duration::from_secs(1),
            browser: false,
            reauthenticate: false,
            cancellation: Cancellation::default(),
        },
    };
    let before = fs::read(&index).unwrap();
    let error = auth
        .recover("main")
        .err()
        .expect("pending evidence failure must propagate");
    assert_eq!(error.code(), "recovery_required");
    assert_eq!(fs::read(index).unwrap(), before);
}

#[test]
fn recovery_failures_preserve_absent_and_present_fields() {
    let error = Error::state_inconsistent("fixture");
    let unavailable = json!({"runtime_usable":false,"snapshot_complete":false,"diagnostic":error});
    assert_eq!(
        crate::runtime::api::value(Recovery::unavailable(error)).to_string(),
        unavailable.to_string()
    );
    let error = Error::state_inconsistent("fixture");
    let failed = json!({"runtime_usable":false,"read_only":true,"snapshot_complete":false,"diagnostic":error,
        "action":"Repair into a new controller with auth repair --reauthenticate --new-state-dir; old state was not modified."});
    assert_eq!(
        crate::runtime::api::value(Recovery::lookup_failed(error)).to_string(),
        failed.to_string()
    );
}

#[test]
fn empty_recovery_retains_explicit_null_projections() {
    let report = RecoveryOutput {
        credential_verification: None,
        operation: AuthOperation::Recover,
        outcome: AuthOutcome::NoRecoveryNeeded,
        profile: "main",
        recovery: None,
    };
    assert_eq!(
        crate::runtime::api::value(report).to_string(),
        json!({
            "operation":"recover","profile":"main","outcome":"no_recovery_needed",
            "recovery":null,"credential_verification":null
        })
        .to_string()
    );
}

#[test]
fn verification_outcomes_keep_their_precedence_over_recovery() {
    let profile = crate::config::Profile {
        provider: crate::config::Provider::Codex,
        method: crate::config::Method::Subscription,
        api_key_env: None,
        api_key_file: None,
    };
    let mut report = Report::new("main", &profile, None);
    report.verification = Some(Verification::Pending);
    report.outcome = Some(AuthOutcome::RepairRequired);
    assert_eq!(
        outcome(Some(&report), false, true),
        AuthOutcome::RepairRequired
    );
    report.outcome = None;
    assert_eq!(
        outcome(Some(&report), false, true),
        AuthOutcome::VerificationPending
    );
    report.verification = None;
    assert_eq!(
        outcome(Some(&report), false, true),
        AuthOutcome::RecoveryFailed
    );
    assert_eq!(outcome(Some(&report), true, true), AuthOutcome::Reconciled);
    assert_eq!(
        outcome(Some(&report), true, false),
        AuthOutcome::NoRecoveryNeeded
    );
}
