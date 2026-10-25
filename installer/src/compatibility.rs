//! Installed parts and recovery records use the sole current installer format.
use hive_input::compatibility::Entry;
pub const INSTALLATION: Entry = Entry::draft("installation");
#[cfg(test)]
pub const FORMATS: &[Entry] = &[INSTALLATION];
pub type Format = hive_input::compatibility::Exact<{ INSTALLATION.current }>;

#[cfg(test)]
#[test]
fn compatibility_registry_matches_contract_inventory() {
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("compatibility/registry.json")).unwrap();
    assert_eq!(serde_json::to_value(FORMATS).unwrap(), expected);
}

#[cfg(test)]
#[test]
fn compatibility_manifest_contract_validates_and_requires_its_marker() {
    let bytes = include_str!("compatibility/manifest.json");
    let value: serde_json::Value = serde_json::from_str(bytes).unwrap();
    let manifest: crate::manifest::Manifest = serde_json::from_str(bytes).unwrap();
    manifest.validate("aarch64-apple-darwin").unwrap();
    assert_eq!(serde_json::to_value(manifest).unwrap(), value);
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
        assert!(serde_json::from_value::<crate::manifest::Manifest>(invalid).is_err());
    }
}
