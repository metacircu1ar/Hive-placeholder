//! Hive-owned data families. Released fingerprints stay empty until declaration.
use hive_input::compatibility::Entry;

pub const EXTERNAL_MCP: &[&str] = &["2025-11-25"];
pub const CLAUDE_MCP: &[&str] = &["2024-11-05"];

pub const STATE: Entry = Entry::draft("state");
pub const SHELL: Entry = Entry::draft("shell_protocol");
pub const CONTROL: Entry = Entry::draft("control_protocol");
pub type AgentCatalog = hive_input::compatibility::Exact<{ AGENT_CATALOG.current }>;
pub const AGENT_CATALOG: Entry = Entry::draft("agent_catalog");
pub const CONTROL_PROTOCOL: u32 = CONTROL.current;
pub const CONTROL_RANGE: hive_input::compatibility::ProtocolRange =
    hive_input::compatibility::ProtocolRange::for_entry(&CONTROL);
pub const COLLECTOR: Entry = Entry::draft("collector");
pub const CONTROLLER_EXPORT: Entry = Entry::draft("controller_export");
pub const FORMATS: &[Entry] = &[
    STATE,
    COLLECTOR,
    CONTROLLER_EXPORT,
    CONTROL,
    AGENT_CATALOG,
    SHELL,
];

#[cfg(test)]
pub(crate) fn contract(name: &str, actual: &impl serde::Serialize) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/compatibility/contracts")
        .join(name);
    let mut bytes = serde_json::to_vec_pretty(actual).unwrap();
    bytes.push(b'\n');
    if std::env::var_os("COMB_RECORD_GOLDENS").is_some() {
        atomic_fs::Publisher::new(
            (hive_setup_support::names::PUBLICATION_OPTIONS).with_mode(0o644),
        )
        .write(&path, &bytes)
        .unwrap();
    }
    assert_eq!(std::fs::read(&path).unwrap(), bytes, "{name}");
}

#[cfg(test)]
#[test]
fn compatibility_registry_matches_contract_inventory() {
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("compatibility/registry.json")).unwrap();
    assert_eq!(serde_json::to_value(FORMATS).unwrap(), expected);
}

pub(crate) fn newer() -> crate::error::Error {
    crate::error::Error::newer_format(
        "This data uses a newer format. The required product release is unknown to \
            this build; keep the data unchanged and use its producing build or a newer \
            supported build.",
    )
}
