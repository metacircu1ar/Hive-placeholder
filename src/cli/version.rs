//! Structured product identity shared by setup and diagnostics.
use super::api;

#[derive(serde::Serialize)]
struct Identity {
    product: &'static str,
    product_version: &'static str,
    commit: Option<&'static str>,
    compiled_features: &'static [&'static str],
    formats: &'static [hive_input::compatibility::Entry],
    shared_formats: SharedFormats,
    mcp_protocols: McpProtocols,
    provider_compatibility: &'static [crate::provider_support::Registry],
}
#[derive(serde::Serialize)]
struct McpProtocols {
    external: &'static [&'static str],
    claude: &'static [&'static str],
}
#[derive(serde::Serialize)]
struct SharedFormats {
    setup: &'static [hive_input::compatibility::Entry],
    #[cfg(feature = "comb")]
    comb_protocol: &'static [hive_input::compatibility::Entry],
}
pub(super) fn identity() -> serde_json::Value {
    api::value(Identity {
        product: "Hive",
        product_version: env!("CARGO_PKG_VERSION"),
        commit: hive_setup_support::identity::COMMIT,
        compiled_features: crate::runtime::features::COMPILED,
        formats: crate::compatibility::FORMATS,
        provider_compatibility: crate::provider_support::REGISTRIES,
        mcp_protocols: McpProtocols {
            external: crate::compatibility::EXTERNAL_MCP,
            claude: crate::compatibility::CLAUDE_MCP,
        },
        shared_formats: SharedFormats {
            setup: hive_setup_support::compatibility::FORMATS,
            #[cfg(feature = "comb")]
            comb_protocol: comb_protocol::compatibility::FORMATS,
        },
    })
}

pub(super) fn run(json: bool) -> i32 {
    let output = if json {
        format!("{:#}\n", identity())
    } else {
        format!("Hive {}\n", crate::runtime::features::VERSION)
    };
    super::parsing::emit_output(output)
}
