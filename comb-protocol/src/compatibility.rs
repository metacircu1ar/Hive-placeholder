//! Shared-file contracts owned by the Comb protocol crate.
use hive_input::compatibility::Entry;
pub const MCP: &[&str] = &["2025-11-25"];
pub const CONNECTION: Entry = Entry::draft("connection");
pub const SERVICE: Entry = Entry::draft("service_protocol");
pub const SERVICE_PROTOCOL: u32 = SERVICE.current;
pub const SERVICE_RANGE: hive_input::compatibility::ProtocolRange =
    hive_input::compatibility::ProtocolRange::for_entry(&SERVICE);
pub const FORMATS: &[Entry] = &[CONNECTION, SERVICE];

#[cfg(test)]
#[test]
fn compatibility_registry_matches_contract_inventory() {
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("compatibility/registry.json")).unwrap();
    assert_eq!(serde_json::to_value(FORMATS).unwrap(), expected);
}
