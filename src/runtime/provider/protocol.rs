//! Provider decision views and wire serializers. Opaque evidence always uses the raw Value.
pub(crate) use crate::runtime::domain::wire_enum;

wire_enum!(ItemKind {
    AgentMessage => "agentMessage", CommandExecution => "commandExecution",
    FileChange => "fileChange", DynamicToolCall => "dynamicToolCall",
});
wire_enum!(UnsubscribeStatus {
    Unsubscribed => "unsubscribed", NotSubscribed => "notSubscribed", NotLoaded => "notLoaded",
});
wire_enum!(AccountKind { Chatgpt => "chatgpt", ApiKey => "apiKey" });
wire_enum!(CoordinationTool { Spawn => "hive_spawn", Batch => "hive_batch" });

// Native catalog request page size, independent of Hive history paging.
pub(crate) const MODEL_PAGE_LIMIT: i32 = 100;

pub(super) mod codes;
mod incoming;
pub(super) mod lenient;
pub(in crate::runtime) use lenient::drift;
mod methods;
pub(crate) mod outgoing;

pub(crate) use incoming::{IncomingFrame, Model, ParamsRef, Response, validate_model};
pub(crate) use methods::{ClientMethod, ServerMethod};

#[cfg(test)]
mod tests;
