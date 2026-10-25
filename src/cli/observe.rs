//! Borrowed, typed polling requests for terminal views.
use crate::runtime::api;
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(super) enum Observation<'a> {
    Attach {
        target: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        before: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        after: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        limit: Option<u64>,
    },
    Live {
        target: &'a str,
        after: &'a Value,
        wait_ms: u64,
    },
    Approval {
        approval_id: &'a str,
    },
    Agents {
        parent: &'a str,
        limit: u64,
        after: u64,
    },
    Usage {
        auth: &'a str,
    },
    Read {
        target: &'a str,
        after: u64,
        limit: u64,
    },
    Tree {
        #[serde(skip_serializing_if = "Option::is_none")]
        root: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        all: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        after: Option<&'a Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        wait_ms: Option<u64>,
    },
}
pub(super) fn value(request: Observation<'_>) -> Value {
    api::value(request)
}
