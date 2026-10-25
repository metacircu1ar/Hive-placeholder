//! Shared setup records outlive the creating executable.
use hive_input::compatibility::Entry;
pub const MAINTENANCE: Entry = Entry::draft("maintenance");
pub const SERVICE: Entry = Entry::draft("service");
pub const FORMATS: &[Entry] = &[MAINTENANCE, SERVICE];

#[cfg(test)]
#[test]
fn compatibility_registry_matches_contract_inventory() {
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("compatibility/registry.json")).unwrap();
    assert_eq!(serde_json::to_value(FORMATS).unwrap(), expected);
}

#[cfg(test)]
#[test]
fn compatibility_shared_record_contracts_require_current_markers() {
    fn check<T: serde::de::DeserializeOwned + serde::Serialize>(bytes: &str) {
        let value: serde_json::Value = serde_json::from_str(bytes).unwrap();
        let native: T = serde_json::from_str(bytes).unwrap();
        assert_eq!(serde_json::to_value(native).unwrap(), value);
        for marker in [
            None,
            Some(serde_json::json!(2)),
            Some(serde_json::Value::Null),
        ] {
            let mut invalid = value.clone();
            invalid.as_object_mut().unwrap().remove("format");
            if let Some(marker) = marker {
                invalid["format"] = marker;
            }
            assert!(serde_json::from_value::<T>(invalid).is_err());
        }
    }
    check::<crate::maintenance::Plan<serde_json::Value>>(include_str!(
        "compatibility/maintenance.json"
    ));
    check::<crate::service::Spec>(include_str!("compatibility/service.json"));
}
