use super::*;

#[test]
fn prune_receipt_requires_exact_fields_and_known_outcome() {
    let receipt = serde_json::json!({
        "account_sessions_revoked": false, "checked_at": 1, "generation": Uuid::nil(),
        "local_credentials_removed": true, "operation": "prune", "outcome": "pruned",
        "profile": "main",
    });
    crate::state::assert_strict_record::<PruneReceipt>(&receipt, &["operation", "outcome"]);
    for (field, bad) in [
        ("profile", serde_json::json!("other")),
        ("generation", serde_json::json!(Uuid::new_v4())),
        ("checked_at", serde_json::json!(0)),
        ("local_credentials_removed", serde_json::json!(false)),
        ("account_sessions_revoked", serde_json::json!(true)),
    ] {
        let mut invalid = receipt.clone();
        invalid[field] = bad;
        let parsed: PruneReceipt = serde_json::from_value(invalid).unwrap();
        assert_eq!(
            parsed.validate("main", Uuid::nil()).unwrap_err().code(),
            "state_inconsistent"
        );
    }
}

mod death;
mod recovery;
mod refusal;
