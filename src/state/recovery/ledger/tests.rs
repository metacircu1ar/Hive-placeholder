//! Require exact execution record fields and known authority states.
use super::{Record, Uuid};

#[test]
fn execution_record_requires_exact_fields_and_known_authority_states() {
    crate::state::assert_strict_record::<Record>(
        &serde_json::json!({
            "never_authorized": true, "conversation": null, "revision": 0,
            "account_identity": null, "id": Uuid::nil(), "generation": Uuid::nil(),
            "run": Uuid::nil(), "boot": Uuid::nil().to_string(),
            "owner": {"pid": 1, "birth": "macos:1:0"}, "lock": {"dev": 1, "ino": 1},
            "class": "provider", "phase": "intent", "process": null,
            "own_group": true, "quiescent": false, "confirmed": false,
        }),
        &["class", "phase"],
    );
}
