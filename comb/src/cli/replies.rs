//! Shared error envelopes, normalized through Value for canonical key order.
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize)]
struct Failure<T> {
    error: T,
}

pub(super) fn failure(error: impl Serialize) -> Value {
    serde_json::json!(Failure { error })
}

#[derive(Serialize)]
pub(super) struct EnvironmentError<'a> {
    pub code: &'a str,
    pub message: String,
}
