//! Approval cards, quota summaries and command-failure presentation fields.
use super::{field, object, present_object, rows};
use crate::error::ErrorCode;
use serde::Deserialize;
use serde_json::Value;

#[derive(Default, Deserialize)]
#[serde(default)]
pub(in crate::cli) struct ErrorReply<'a> {
    #[serde(borrow, deserialize_with = "object")]
    pub error: Failure<'a>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(in crate::cli) struct Failure<'a> {
    #[serde(deserialize_with = "field")]
    pub code: Option<ErrorCode>,
    #[serde(borrow, deserialize_with = "field")]
    pub message: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub field: Option<&'a str>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(in crate::cli) struct Approval<'a> {
    #[serde(borrow, deserialize_with = "field")]
    pub id: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub state: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub title: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub message: Option<&'a str>,
    #[serde(deserialize_with = "field")]
    pub destructive: bool,
    pub schema: Option<Value>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(in crate::cli) struct UsageReport<'a> {
    #[serde(borrow, deserialize_with = "field")]
    pub auth: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub generation: Option<&'a str>,
    #[serde(deserialize_with = "field")]
    pub observed_at: Option<u64>,
    #[serde(borrow, deserialize_with = "rows")]
    pub windows: Vec<Window<'a>>,
    #[serde(borrow, deserialize_with = "present_object")]
    pub credits: Option<Credits<'a>>,
    #[serde(borrow, deserialize_with = "present_object")]
    pub extra_usage: Option<ExtraUsage<'a>>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(in crate::cli) struct Window<'a> {
    #[serde(borrow, deserialize_with = "field")]
    pub name: Option<&'a str>,
    #[serde(deserialize_with = "field")]
    pub used_percent: f64,
    #[serde(deserialize_with = "field")]
    pub minutes: Option<u64>,
    #[serde(deserialize_with = "field")]
    pub resets_at: Option<u64>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(in crate::cli) struct Credits<'a> {
    #[serde(deserialize_with = "field")]
    pub unlimited: bool,
    #[serde(rename = "has_credits", deserialize_with = "field")]
    pub available: bool,
    #[serde(borrow, deserialize_with = "field")]
    pub balance: Option<&'a str>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(in crate::cli) struct ExtraUsage<'a> {
    #[serde(deserialize_with = "field")]
    pub in_use: bool,
    #[serde(borrow, deserialize_with = "field")]
    pub status: Option<&'a str>,
}
