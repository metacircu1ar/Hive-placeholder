//! Account-probe requests have a smaller handshake than runtime provider sessions.
use crate::runtime::api::value as project;
use crate::runtime::provider_requests as outgoing;
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize)]
#[serde(tag = "method")]
enum Request {
    #[serde(rename = "initialize")]
    Initialize { id: u64, params: Initialize },
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Initialize {
    client_info: ClientInfo,
}
#[derive(Serialize)]
struct ClientInfo {
    name: &'static str,
    version: &'static str,
}
pub(super) fn initialize() -> Value {
    project(Request::Initialize {
        id: super::INITIALIZE_REQUEST_ID,
        params: Initialize {
            client_info: ClientInfo {
                name: "hive",
                version: env!("CARGO_PKG_VERSION"),
            },
        },
    })
}
pub(super) fn initialized() -> Value {
    project(outgoing::initialized())
}
pub(super) fn account_read() -> Value {
    project(outgoing::request(
        super::ACCOUNT_REQUEST_ID,
        outgoing::account_read(),
    ))
}

#[cfg(test)]
mod tests;
