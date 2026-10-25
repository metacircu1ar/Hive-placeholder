//! Name provider methods and their routing and input-submission properties.
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Visitor};
use std::fmt;

macro_rules! methods {
    ($name:ident {$($variant:ident => $wire:literal),+ $(,)?}) => {
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub(crate) enum $name {
            $($variant,)+
            Other(String),
        }
        impl $name {
            pub(crate) fn parse(value: &str) -> Self {
                match value { $($wire => Self::$variant,)+ other => Self::Other(other.into()) }
            }
            pub(crate) fn as_str(&self) -> &str {
                match self { $(Self::$variant => $wire,)+ Self::Other(value) => value }
            }
        }
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct MethodVisitor;
                impl Visitor<'_> for MethodVisitor {
                    type Value = $name;
                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str("a provider method string")
                    }
                    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                        Ok($name::parse(value))
                    }
                }
                deserializer.deserialize_str(MethodVisitor)
            }
        }
    };
}

macro_rules! client_methods {
    ($($variant:ident => $wire:literal),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(crate) enum ClientMethod { $($variant,)+ }
        impl ClientMethod {
            pub(crate) const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $wire,)+ }
            }
            pub(crate) fn is_cleanup(self) -> bool {
                matches!(
                    self,
                    Self::TurnInterrupt | Self::TerminalClean
                        | Self::TerminalList | Self::ThreadUnsubscribe | Self::ThreadLoadedList
                )
            }
            pub(crate) fn opens_thread(self) -> bool {
                matches!(self, Self::ThreadStart | Self::ThreadResume | Self::ThreadFork)
            }
            pub(crate) fn submits_input(self) -> bool {
                self.opens_thread() || self == Self::TurnStart
            }
            pub(crate) fn submission_error(self,
                 multiplexed: bool,
                 error: crate::error::Error) -> crate::error::Error {
                if self == Self::ThreadFork || !multiplexed && self == Self::TurnStart {
                    error.before_submission()
                } else { error }
            }
        }
        impl Serialize for ClientMethod {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

    };
}

client_methods! {
    Initialize => "initialize",
    Initialized => "initialized",
    AccountRead => "account/read",
    ApiLogin => "account/login/start",
    ConfigRequirements => "configRequirements/read",
    ConfigRead => "config/read",
    ModelList => "model/list",
    ThreadLoadedList => "thread/loaded/list",
    RateLimitsRead => "account/rateLimits/read",
    ThreadStart => "thread/start",
    ThreadResume => "thread/resume",
    ThreadFork => "thread/fork",
    TurnStart => "turn/start",
    TurnSteer => "turn/steer",
    TurnInterrupt => "turn/interrupt",
    ThreadUnsubscribe => "thread/unsubscribe",
    TerminalList => "thread/backgroundTerminals/list",
    TerminalClean => "thread/backgroundTerminals/clean",
}

methods!(ServerMethod {
    ThreadStarted => "thread/started",
    ThreadStatusChanged => "thread/status/changed",
    ThreadClosed => "thread/closed",
    TurnStarted => "turn/started",
    TurnCompleted => "turn/completed",
    ItemStarted => "item/started",
    ItemCompleted => "item/completed",
    AgentMessageDelta => "item/agentMessage/delta",
    ReasoningTextDelta => "item/reasoning/textDelta",
    ReasoningSummaryDelta => "item/reasoning/summaryTextDelta",
    CommandOutputDelta => "item/commandExecution/outputDelta",
    ToolCall => "item/tool/call",
    ModelRerouted => "model/rerouted",
    AccountUpdated => "account/updated",
    RateLimitsUpdated => "account/rateLimits/updated",
    ClaudeRateLimits => "hive/claudeRateLimits",
    ConfigurationWarning => "configWarning",
    Error => "error",
});

impl ServerMethod {
    pub(crate) fn is_item(&self) -> bool {
        // Unknown item methods still participate in lifecycle observation. Deltas
        // are live-only text and never item lifecycle evidence.
        self.as_str().starts_with("item/") && !self.is_delta()
    }
    pub(crate) fn is_delta(&self) -> bool {
        matches!(
            self,
            Self::AgentMessageDelta
                | Self::ReasoningTextDelta
                | Self::ReasoningSummaryDelta
                | Self::CommandOutputDelta
        )
    }
}
