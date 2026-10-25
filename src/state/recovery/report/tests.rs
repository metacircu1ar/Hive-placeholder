use super::*;
use serde_json::json;

#[test]
fn typed_receipts_keep_exact_fields_nulls_and_sorted_bytes()
-> Result<(), Box<dyn std::error::Error>> {
    let generation = Uuid::from_u128(1);
    let execution_id = Uuid::from_u128(2);
    let credential_lock = LockIdentity::from_fd(tempfile::tempfile()?)?;
    let expected = json!({
        "generation":generation,"checked_at":123,"credential_lock":credential_lock,
        "runtime_usable":true,"account_identity_available":false,"history_error":null,
        "snapshot_current":false,"executions":[{
            "execution_id":execution_id,"revision":7,"class":"credential_writer",
            "phase":"authorized","ownership_gone":false,"conversation_safe":true,
            "confirmed":false,"quiescent":true
        }]
    });
    let receipt = Reconciliation {
        account_identity_available: false,
        checked_at: 123,
        credential_lock,
        executions: vec![Observation {
            class: Class::CredentialWriter,
            completion: Completion {
                confirmed: false,
                quiescent: true,
            },
            conversation_safe: true,
            execution_id,
            ownership_gone: false,
            phase: ExecutionPhase::Authorized,
            revision: 7,
        }],
        generation,
        history_error: None,
        runtime_usable: true,
        snapshot_current: false,
    };
    let actual = crate::runtime::api::value(&receipt);
    assert_eq!(actual, expected);
    assert_eq!(serde_json::to_vec(&actual)?, serde_json::to_vec(&expected)?);
    assert!(RecoveryReport::Snapshot(receipt).runtime_usable());
    Ok(())
}

#[test]
fn diagnostic_reports_keep_their_existing_failure_shape() {
    let generation = Uuid::from_u128(1);
    let error = Error::state_inconsistent("fixture");
    let expected = json!({
        "generation":generation,"runtime_usable":false,"read_only":true,
        "snapshot_complete":false,"executions":[],"diagnostic":error,
        "action":"Use auth repair --reauthenticate for isolated credentials; use --new-state-dir if controller ownership is damaged. No old evidence was changed."
    });
    let report = RecoveryReport::diagnostic(generation, error);
    assert!(!report.runtime_usable());
    assert_eq!(
        crate::runtime::api::value(report).to_string(),
        expected.to_string()
    );
}
