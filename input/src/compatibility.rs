//! Side-effect-free descriptions of exact owned formats; no implicit adoption.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Released {
    pub product_version: &'static str,
    pub number: u32,
    pub fingerprint: &'static str,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Entry {
    pub name: &'static str,
    pub current: u32,
    pub minimum: u32,
    pub maximum: u32,
    pub released: &'static [Released],
}
impl Entry {
    pub const fn draft(name: &'static str) -> Self {
        Self {
            name,
            current: 1,
            minimum: 1,
            maximum: 1,
            released: &[],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Marker {
    pub format: u32,
}
impl Marker {
    pub const fn current(entry: &Entry) -> Self {
        Self {
            format: entry.current,
        }
    }
}

/// Negotiable protocol interval. Validate received intervals before selecting a
/// common version; an empty intersection never authorizes a mutation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ProtocolRange {
    pub minimum: u32,
    pub maximum: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RangeRecord {
    minimum: u32,
    maximum: u32,
}
impl<'de> Deserialize<'de> for ProtocolRange {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Object;
        impl<'de> serde::de::Visitor<'de> for Object {
            type Value = ProtocolRange;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a protocol range object")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                map: M,
            ) -> Result<Self::Value, M::Error> {
                let raw =
                    RangeRecord::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
                raw.try_into().map_err(serde::de::Error::custom)
            }
        }
        d.deserialize_map(Object)
    }
}
impl TryFrom<RangeRecord> for ProtocolRange {
    type Error = &'static str;
    fn try_from(value: RangeRecord) -> Result<Self, Self::Error> {
        if value.minimum == 0 || value.minimum > value.maximum {
            return Err("Invalid protocol range");
        }
        Ok(Self {
            minimum: value.minimum,
            maximum: value.maximum,
        })
    }
}
impl ProtocolRange {
    pub const fn for_entry(entry: &Entry) -> Self {
        Self {
            minimum: entry.minimum,
            maximum: entry.maximum,
        }
    }
    pub fn select(self, other: Self) -> Option<u32> {
        let minimum = self.minimum.max(other.minimum);
        let maximum = self.maximum.min(other.maximum);
        (minimum > 0 && minimum <= maximum).then_some(maximum)
    }
    pub fn contains(self, protocol: u32) -> bool {
        protocol >= self.minimum && protocol <= self.maximum
    }
}

/// MCP dates belong to the external protocol, independently of `Hive` format IDs.
pub fn select_mcp(requested: &str, supported: &'static [&'static str]) -> Option<&'static str> {
    if requested.is_empty() || requested.len() > 64 || requested.chars().any(char::is_control) {
        return None;
    }
    supported
        .iter()
        .copied()
        .find(|value| *value == requested)
        .or_else(|| supported.first().copied())
}

/// Required numeric identity for a single supported shared-file shape. Missing,
/// null and different numbers are errors; construction always emits N.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Exact<const N: u32>;
impl<const N: u32> Serialize for Exact<N> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(N)
    }
}
impl<'de, const N: u32> Deserialize<'de> for Exact<N> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if u32::deserialize(deserializer)? != N {
            return Err(serde::de::Error::custom("Unsupported shared-file format"));
        }
        Ok(Self)
    }
}

#[cfg(test)]
#[test]
fn protocol_negotiation_selects_only_a_valid_common_shape() {
    let current = ProtocolRange {
        minimum: 1,
        maximum: 1,
    };
    assert_eq!(
        current.select(ProtocolRange {
            minimum: 1,
            maximum: 2
        }),
        Some(1)
    );
    assert_eq!(
        current.select(ProtocolRange {
            minimum: 2,
            maximum: 3
        }),
        None
    );
    for invalid in [
        "[1,1]",
        r#"{"minimum":0,"maximum":1}"#,
        r#"{"minimum":2,"maximum":1}"#,
        r#"{"minimum":1,"maximum":1,"extra":false}"#,
        r#"{"minimum":1,"minimum":1,"maximum":1}"#,
    ] {
        assert!(serde_json::from_str::<ProtocolRange>(invalid).is_err());
    }
    assert_eq!(
        select_mcp("2025-11-25", &["2025-11-25"]),
        Some("2025-11-25")
    );
    assert_eq!(select_mcp("future", &["2024-11-05"]), Some("2024-11-05"));
    assert_eq!(select_mcp("", &["2025-11-25"]), None);
    assert_eq!(select_mcp("2025-11-25", &[]), None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct Record {
        format: Exact<1>,
        value: String,
    }

    #[test]
    fn exact_marker_is_required_and_does_not_accept_coercions() {
        let current = r#"{"format":1,"value":"kept"}"#;
        let record: Record = serde_json::from_str(current).unwrap();
        assert_eq!(serde_json::to_string(&record).unwrap(), current);
        for invalid in [
            r#"{"value":"kept"}"#,
            r#"{"format":null,"value":"kept"}"#,
            r#"{"format":"1","value":"kept"}"#,
            r#"{"format":1.0,"value":"kept"}"#,
            r#"{"format":0,"value":"kept"}"#,
            r#"{"format":2,"value":"kept"}"#,
            r#"{"format":-1,"value":"kept"}"#,
            r#"{"format":4294967296,"value":"kept"}"#,
            r#"{"format":1,"format":1,"value":"kept"}"#,
        ] {
            assert!(
                serde_json::from_str::<Record>(invalid).is_err(),
                "{invalid}"
            );
        }
    }
}
