use super::*;
use serde_json::{Value, json};

#[test]
fn flat_probe_records_require_one_complete_identity() {
    let base = serde_json::to_value(Availability::available("main", uuid::Uuid::nil())).unwrap();
    let fields = [
        ("probe_id", json!("lease")),
        ("probe_agent", json!("agent")),
        ("probe_message", json!("input")),
        ("probe_deadline", json!(123)),
        ("probe_error", json!("missing_owner")),
    ];
    for mask in 0..32 {
        let mut record = base.clone();
        for (bit, (key, value)) in fields.iter().enumerate() {
            if mask & (1 << bit) != 0 {
                record[key] = value.clone();
            }
        }
        let parsed = serde_json::from_value::<Availability>(record.clone());
        assert_eq!(parsed.is_ok(), matches!(mask, 0 | 15 | 31), "{mask}");
        if let Ok(value) = parsed {
            assert_eq!(serde_json::to_value(value).unwrap(), record);
        }
    }
    for (field, _) in fields {
        let mut record = base.clone();
        record.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<Availability>(record).is_err(),
            "{field}"
        );
    }
    let mut unknown = base.clone();
    unknown["unknown"] = Value::Null;
    assert!(serde_json::from_value::<Availability>(unknown).is_err());
    let positional = Value::Array(base.as_object().unwrap().values().cloned().collect());
    assert!(serde_json::from_value::<Availability>(positional).is_err());
}

#[test]
fn availability_reset_keeps_probe_ownership_and_evidence_clock() {
    let mut value = Availability::available("main", uuid::Uuid::nil());
    value.state = AvailabilityState::Limited;
    value.reason = Some(AuthReason::UsageLimit);
    value.until = Some(100);
    value.retry_at = Some(200);
    value.window = Some("primary".into());
    value.probe = Some(Probe {
        id: "p".into(),
        agent: "a".into(),
        message: "m".into(),
        deadline: 300,
        error: None,
    });
    let probe = value.probe.clone();
    let updated_at = value.updated_at;
    value.mark_available();
    assert_eq!(value.state, AvailabilityState::Available);
    assert!(
        value.reason.is_none()
            && value.until.is_none()
            && value.retry_at.is_none()
            && value.window.is_none()
    );
    assert_eq!(value.probe, probe);
    assert_eq!(value.updated_at, updated_at);
    value.clear_probe();
    assert!(value.probe.is_none());
}
