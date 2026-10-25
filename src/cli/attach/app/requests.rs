//! Typed view commands, serialized through Value for canonical object order.
use crate::runtime::api;
use serde::Serialize;
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

#[derive(Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(super) enum Request<'a> {
    Send {
        target: &'a str,
        body: &'a str,
        mode: &'a str,
    },
    SetName {
        target: &'a str,
        name: &'a str,
    },
    SetAuth {
        target: &'a str,
        auth: &'a str,
    },
    Complete {
        target: &'a str,
        body: &'a str,
    },
    Spawn {
        #[serde(skip_serializing_if = "Option::is_none")]
        from_agent: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        role: Option<&'a str>,
        body: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        cwd: Option<&'a Path>,
    },
    Respond {
        approval_id: &'a str,
        action: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<Value>,
    },
}
pub(super) fn value(request: Request<'_>) -> Value {
    api::value(request)
}

#[derive(Serialize)]
struct Control<'a> {
    op: &'a str,
    target: &'a str,
}
pub(super) fn control(op: &str, target: &str) -> Value {
    api::value(Control { op, target })
}

#[derive(Serialize)]
struct OptionChange<'a, T> {
    op: &'static str,
    target: &'a str,
    options: T,
}
pub(super) fn options(target: &str, options: impl Serialize) -> Value {
    api::value(OptionChange {
        op: "set_options",
        target,
        options,
    })
}
pub(super) fn option(target: &str, key: &str, value: &str) -> Value {
    options(target, BTreeMap::from([(key, value)]))
}
