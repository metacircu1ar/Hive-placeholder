//! Trusted operation examples for deferred input consumption.
use crate::error::Error;
use hive_input::Code;

#[derive(Clone, Copy)]
pub(crate) enum InputContext {
    Spawn,
    Send,
    Complete,
    Terminate,
    Stop,
    Resume,
    Interrupt,
    Purge,
    Gc,
    SetOptions,
    OptionsCatalog,
    SetAuth,
    SetName,
    LoginRestored,
    AuthAvailability,
    Usage,
    Status,
    Agents,
    Read,
    ReadJoin,
    Messages,
    Output,
    MessageResult,
    Live,
    Tree,
    Attach,
    Control,
    #[cfg(feature = "comb")]
    CollectorDiscard,
    #[cfg(feature = "comb")]
    CollectorRelease,
    #[cfg(feature = "comb")]
    CollectorRebind,
}
impl InputContext {
    pub(crate) fn example(self) -> &'static str {
        match self {
            Self::Spawn => {
                r#"{"op":"spawn","role":"reviewer","cwd":"/workspace","body":"Review the work."}"#
            }
            Self::Send => r#"{"op":"send","target":"AGENT_ID","body":"Review the work."}"#,
            Self::Complete => {
                r#"{"op":"complete","target":"AGENT_ID","body":"Implemented and validated."}"#
            }
            Self::Terminate => r#"{"op":"terminate","target":"AGENT_ID"}"#,
            Self::Stop => r#"{"op":"stop","target":"AGENT_ID"}"#,
            Self::Resume => r#"{"op":"resume","target":"AGENT_ID"}"#,
            Self::Interrupt => r#"{"op":"interrupt","target":"AGENT_ID"}"#,
            Self::Purge => r#"{"op":"purge","target":"AGENT_ID","dry_run":true}"#,
            Self::Gc => r#"{"op":"gc","before":1,"dry_run":true}"#,
            Self::OptionsCatalog => r#"{"op":"options_catalog","provider":"codex"}"#,
            Self::SetOptions => {
                "{\"op\":\"set_options\",\"target\":\"AGENT_ID\",\"options\":{\"model\":\
                \"gpt-6-luna\",\"effort\":\"low\"}}"
            }
            Self::SetAuth => r#"{"op":"set_auth","target":"AGENT_ID","auth":"codex-main"}"#,
            Self::SetName => r#"{"op":"set_name","target":"AGENT_ID","name":"reviewer"}"#,
            Self::LoginRestored => r#"{"op":"login_restored","auth":"codex-main"}"#,
            Self::AuthAvailability => r#"{"op":"auth_availability","auth":"codex-main"}"#,
            Self::Usage => r#"{"op":"usage","auth":"codex-main"}"#,
            Self::Status => r#"{"op":"status","target":"AGENT_ID"}"#,
            Self::Agents => r#"{"op":"agents","limit":100}"#,
            Self::Read => r#"{"op":"read","target":"AGENT_ID","limit":100}"#,
            Self::ReadJoin => r#"{"op":"read","target":"join:JOIN_ID"}"#,
            Self::Messages => r#"{"op":"messages","target":"AGENT_ID","limit":100}"#,
            Self::Output => {
                r#"{"op":"output","id":"00000000-0000-4000-8000-000000000001","limit":65536}"#
            }
            Self::MessageResult => r#"{"op":"message_result","message_id":"MESSAGE_ID"}"#,
            Self::Live => {
                "{\"op\":\"live\",\"target\":\"AGENT_ID\",\"after\":{\"epoch\":1,\
                \"generation\":1,\"seq\":0,\"durable\":0},\"wait_ms\":25000}"
            }
            Self::Tree => r#"{"op":"tree","root":"AGENT_ID","wait_ms":25000}"#,
            Self::Attach => r#"{"op":"attach","target":"AGENT_ID","limit":100}"#,
            #[cfg(feature = "comb")]
            Self::CollectorDiscard => {
                r#"{"op":"collector_admin","action":"discard","destination":"project"}"#
            }
            #[cfg(feature = "comb")]
            Self::CollectorRelease => {
                "{\"op\":\"collector_admin\",\"action\":\"release_uncertain\",\"destination\":\
                \"project\",\"item\":\"ITEM_ID\"}"
            }
            #[cfg(feature = "comb")]
            Self::CollectorRebind => {
                "{\"op\":\"collector_admin\",\"action\":\"rebind\",\"destination\":\"project\",\
                \"comb_id\":\"00000000-0000-4000-8000-000000000001\"}"
            }
            Self::Control => {
                r#"{"protocol":1,"id":"request-1","args":{"op":"status","target":"AGENT_ID"}}"#
            }
        }
    }
    pub(crate) fn error(self, code: Code, field: &str, expected: &str) -> Error {
        hive_input::InputError::new(
            code,
            "Invalid operation arguments.",
            field,
            expected,
            vec![self.example().into()],
        )
        .map_or_else(
            |cause| Error::internal_error("Invalid input-error metadata.").with_source(cause),
            Error::from,
        )
    }
}
