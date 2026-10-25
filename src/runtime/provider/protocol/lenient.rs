//! Represent malformed provider fields without rejecting unrelated evidence.
// Serde field hooks require Result even when malformed values intentionally become absent.
#![allow(clippy::unnecessary_wraps)]
use serde::{Deserialize, Deserializer};
use std::sync::atomic::AtomicU64;

static DRIFT: AtomicU64 = AtomicU64::new(0);

pub(in crate::runtime) fn drift(
    phase: &'static str,
    error: Option<&(dyn std::error::Error + 'static)>,
) {
    if let Some(count) = crate::observability::sample(&DRIFT) {
        if let Some(error) = error {
            tracing::debug!(name: "provider.protocol_drift",
                 phase = phase,
                 count = count,
                 error = error,
                 decision = "retain_other_evidence");
        } else {
            // Generic serde field errors may borrow input. Do not require a
            // 'static error or render it just to extract diagnostic metadata.
            tracing::debug!(name: "provider.protocol_drift",
                 phase = phase,
                 count = count,
                 code = "invalid_field",
                 decision = "retain_other_evidence");
        }
    }
}

// Each field consumes its own value even when its type is wrong. A malformed sibling
// must not erase fields that the untyped runtime previously read successfully.
pub(crate) fn optional<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(T::deserialize(deserializer)
        .inspect_err(|_| drift("optional_field", None))
        .ok())
}

pub(crate) fn defaulting<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    // Positional arrays must never reach a struct visitor.
    Ok(crate::runtime::domain::decode::object(deserializer)
        .inspect_err(|_| drift("defaulted_object", None))
        .unwrap_or_default())
}

pub(crate) fn elements<'de, D, T>(deserializer: D) -> Result<Option<Vec<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    #[derive(Deserialize)]
    #[serde(bound(deserialize = "T: Deserialize<'de> + Default"))]
    struct Element<T>(#[serde(deserialize_with = "defaulting")] T);
    Ok(optional::<D, Vec<Element<T>>>(deserializer)?
        .map(|values| values.into_iter().map(|value| value.0).collect()))
}
