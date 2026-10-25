//! Carry request identity and typed parameters through routing to final serialization.
use super::{
    ClientMethod,
    params::{
        AccountRead, Empty, Initialize, Interrupt, LoadedThreadPage, ModelPage, Steer, Thread,
        ThreadFork, ThreadResume, ThreadStart, TurnStart,
    },
};
use serde::Serialize;

#[derive(Clone, Serialize)]
// Only parameters serialize here; OutgoingFrame attaches the method from this enum.
#[serde(untagged)]
pub(crate) enum OutgoingRequest {
    Initialize(Initialize),
    AccountRead(AccountRead),
    ApiLogin(crate::auth::ApiLogin),
    ConfigRequirements(Empty),
    ConfigRead(Empty),
    ModelList(ModelPage),
    ThreadLoadedList(LoadedThreadPage),
    RateLimitsRead(Empty),
    ThreadStart(ThreadStart),
    ThreadResume(ThreadResume),
    ThreadFork(ThreadFork),
    TurnStart(TurnStart),
    TurnSteer(Steer),
    TurnInterrupt(Interrupt),
    ThreadUnsubscribe(Thread),
    TerminalList(Thread),
    TerminalClean(Thread),
}
impl OutgoingRequest {
    pub(crate) const fn method(&self) -> ClientMethod {
        match self {
            Self::Initialize(_) => ClientMethod::Initialize,
            Self::AccountRead(_) => ClientMethod::AccountRead,
            Self::ApiLogin(_) => ClientMethod::ApiLogin,
            Self::ConfigRequirements(_) => ClientMethod::ConfigRequirements,
            Self::ConfigRead(_) => ClientMethod::ConfigRead,
            Self::ModelList(_) => ClientMethod::ModelList,
            Self::ThreadLoadedList(_) => ClientMethod::ThreadLoadedList,
            Self::RateLimitsRead(_) => ClientMethod::RateLimitsRead,
            Self::ThreadStart(_) => ClientMethod::ThreadStart,
            Self::ThreadResume(_) => ClientMethod::ThreadResume,
            Self::ThreadFork(_) => ClientMethod::ThreadFork,
            Self::TurnStart(_) => ClientMethod::TurnStart,
            Self::TurnSteer(_) => ClientMethod::TurnSteer,
            Self::TurnInterrupt(_) => ClientMethod::TurnInterrupt,
            Self::ThreadUnsubscribe(_) => ClientMethod::ThreadUnsubscribe,
            Self::TerminalList(_) => ClientMethod::TerminalList,
            Self::TerminalClean(_) => ClientMethod::TerminalClean,
        }
    }
    pub(crate) fn thread(&self) -> Option<&str> {
        match self {
            Self::ThreadResume(params) => Some(&params.thread_id),
            Self::ThreadFork(params) => Some(&params.thread_id),
            Self::TurnStart(params) => Some(&params.thread_id),
            Self::TurnSteer(params) => Some(&params.thread_id),
            Self::TurnInterrupt(params) => Some(&params.thread_id),
            Self::ThreadUnsubscribe(params)
            | Self::TerminalList(params)
            | Self::TerminalClean(params) => Some(&params.thread_id),
            _ => None,
        }
    }
    pub(crate) fn expected_turn(&self) -> Option<&str> {
        match self {
            Self::TurnSteer(params) => Some(&params.expected_turn_id),
            _ => None,
        }
    }
    pub(crate) fn interrupted_turn(&self) -> Option<&str> {
        match self {
            Self::TurnInterrupt(params) => Some(&params.turn_id),
            _ => None,
        }
    }
    pub(crate) fn fork_cutoff(&self) -> Option<&str> {
        match self {
            Self::ThreadFork(params) => Some(&params.last_turn_id),
            _ => None,
        }
    }
}
