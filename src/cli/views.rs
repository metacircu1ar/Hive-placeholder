//! Typed presentation projections of already parsed daemon replies.
//!
//! An unavailable field keeps its display default independently of neighboring
//! fields. Decoding runs against `Value`, after transport validation, so a bad
//! field cannot leave an input stream partially consumed.
mod agents;
mod events;
pub(super) use agents::{Agent, Attachment, Tree};
pub(super) use events::{Event, LiveItem, Page, Payload, ProviderItem, Snapshot};
mod traffic;
pub(super) use traffic::{
    AgentIdentity, ArtifactOutput, AttachMetadata, Detail, Listing, LivePage, RawAttachment,
    TerminalPane, TreeMetadata,
};
mod metadata;
pub(super) use metadata::{Approval, Credits, ExtraUsage, Failure, UsageReport, Window};

use crate::error::ErrorCode;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

pub(super) fn read<'de, T: Deserialize<'de> + Default>(value: &'de Value) -> T {
    crate::runtime::domain::decode::object(value).unwrap_or_default()
}

pub(super) fn list<'de, T: Deserialize<'de> + Default>(value: &'de Value) -> Vec<T> {
    rows(value).unwrap_or_default()
}

pub(super) fn present_object<'de, D, T>(input: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    object(input).map(Some)
}

pub(super) fn field<'de, D, T>(input: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    T::deserialize(input).or_else(|_| Ok(T::default()))
}

pub(super) fn object<'de, D, T>(input: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    crate::runtime::domain::decode::object(input).or_else(|_| Ok(T::default()))
}

pub(super) fn rows<'de, D, T>(input: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    struct Row<T>(T);
    impl<'de, T: Deserialize<'de> + Default> Deserialize<'de> for Row<T> {
        fn deserialize<D: Deserializer<'de>>(input: D) -> Result<Self, D::Error> {
            object(input).map(Self)
        }
    }
    Vec::<Row<T>>::deserialize(input)
        .or_else(|_| Ok(Vec::new()))
        .map(|rows| rows.into_iter().map(|Row(row)| row).collect())
}

pub(super) fn present<'de, D: Deserializer<'de>>(input: D) -> Result<bool, D::Error> {
    Value::deserialize(input).map(|value| !value.is_null())
}

pub(super) fn error_code(reply: &Value) -> ErrorCode {
    read::<metadata::ErrorReply<'_>>(reply)
        .error
        .code
        .unwrap_or(ErrorCode::Internal)
}

#[cfg(test)]
mod tests;
