//! Comb's current root and derived-data contracts, before the first release.
use hive_input::compatibility::Entry;
pub const ROOT: Entry = Entry::draft("root");
pub const SEAL: Entry = Entry::draft("seal");
pub const INDEX: Entry = Entry::draft("index");
pub const VECTORS: Entry = Entry::draft("vectors");
pub const FORMATS: &[Entry] = &[ROOT, SEAL, INDEX, VECTORS];

#[cfg(test)]
#[test]
fn compatibility_registry_matches_contract_inventory() {
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("compatibility/registry.json")).unwrap();
    assert_eq!(serde_json::to_value(FORMATS).unwrap(), expected);
}
