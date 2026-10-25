//! Shared operation types and adapters for the three request boundaries.
mod approvals;
pub(crate) mod batch;
pub(crate) mod events;
mod input;
pub(crate) mod joins;
pub(crate) mod pagination;
mod request;
mod responses;
pub(in crate::runtime) mod selection;
mod stalls;
mod tools;
pub(crate) use stalls::{STALLED_PREVIEW, StalledCursor};
pub(crate) use stalls::{StalledGroup, StalledPage};

pub(crate) use approvals::{
    ApprovalAction, ApprovalArgs, ApprovalCursor, ApprovalInput, ApprovalOptions, ApprovalsArgs,
    AskArgs, AskContent, RespondArgs,
};
pub(crate) use input::{Input, InputContext};
pub(crate) use request::{
    AgentsArgs, AttachArgs, AuthArgs, BatchArgs, BatchItem, BatchKind, BatchOperation,
    CompleteArgs, GcArgs, GcCursor, GcOptions, KeySourceArgs, LiveArgs, MessageResultArgs,
    MessagesArgs, OptionsCatalogArgs, OutputArgs, PurgeArgs, PurgeOptions, ReadArgs, Request,
    RequestEnvelope, SendArgs, SetAuthArgs, SetNameArgs, SetOptionsArgs, SpawnArgs, StatusArgs,
    TargetArgs, TreeArgs, UsageArgs,
};
pub(crate) use responses::{
    AgentResponse, AgentsPage, AttachReply, AttachResponse, AttachedMessage, AuthAvailability,
    AuthChangedResponse, AuthGroup, AuthGroups, AuthMember, BatchResponse, CallOutcome,
    Capabilities, CombBacklog, CompleteResponse, ControlReply, ControlResponse, DaemonReply,
    ErrorResponse, EventRecord, EventsPage, GC_PREVIEW, GcPage, GcStats, HistoryExpiredResponse,
    HistoryPage, ListingFields, MessageRecord, MessageResult, MessagesPage, ModelsResponse,
    NameChangedResponse, OptionsChangedResponse, OutputResponse, OutputSelection, PingResponse,
    ProviderCapabilities, PurgeBlocker, PurgeCounts, PurgeResponse, RedirectTarget, RequiredBuild,
    RolesResponse, RuntimeResources, SendResponse, SpawnResponse, StatusResponse, SteeringPolicy,
    SteeringTarget, SteeringVerification, StorageDiagnostics, TerminateResponse, TreeReply,
    TreeResponse, TreeRow, UsageReply, VerificationSelection, WaitEventResponse,
    WaitObservationResponse, WriterObservation, listed_agent, value,
};
#[cfg(feature = "comb")]
pub(crate) use responses::{CollectorReply, ValueError};
pub(crate) use tools::{
    contextualize as tool_input_context, error as tool_input_error, tools, validate_tool,
};

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Test fixtures and assertions deliberately panic on failure.
mod tests;
