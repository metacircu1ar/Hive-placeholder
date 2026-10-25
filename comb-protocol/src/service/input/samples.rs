//! Typed, trusted protocol examples; Value keeps their canonical key order.
use serde::Serialize;
use serde_json::Value;

// One deliberate JSON builder retains the existing infallible API for trusted
// examples and generated schemas. Callers supply named Serialize shapes.
pub(in crate::service) fn value(input: impl Serialize) -> Value {
    serde_json::json!(input)
}

#[derive(Serialize)]
#[serde(untagged)]
pub(super) enum ReadExample {
    Search {
        query: &'static str,
        mode: &'static str,
        limit: usize,
    },
    List {
        prefix: &'static str,
        limit: usize,
    },
    Slug {
        slug: &'static str,
    },
    Links {
        slug: &'static str,
        limit: usize,
    },
    Graph {
        slug: &'static str,
        depth: usize,
    },
}

#[derive(Serialize)]
pub(super) struct RetrievalOperation<'a> {
    pub op: &'a str,
    pub request: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<Value>,
}

#[derive(Serialize)]
#[serde(untagged)]
pub(super) enum AdminExample {
    Import {
        op: &'static str,
        directory: &'static str,
        prefix: &'static str,
    },
    Batch {
        op: &'static str,
        batch_id: &'static str,
    },
    Create {
        op: &'static str,
        grant: ClientGrant,
    },
    Revoke {
        op: &'static str,
        id: &'static str,
    },
    Simple {
        op: &'static str,
    },
}
#[derive(Serialize)]
pub(super) struct ClientGrant {
    pub kind: &'static str,
    pub name: &'static str,
    pub reviewer: bool,
}

#[derive(Serialize)]
#[serde(untagged)]
pub(super) enum WriteExample<'a> {
    Put {
        slug: &'static str,
        markdown: &'static str,
        request_id: &'static str,
    },
    Append {
        slug: &'static str,
        text: &'static str,
        source: &'static str,
        request_id: &'static str,
    },
    Capture {
        text: &'static str,
        request_id: &'static str,
    },
    Consolidate {
        note: &'static str,
        target: &'static str,
        note_hash: &'a str,
        target_hash: &'a str,
        summary: &'static str,
        run: &'static str,
        request_id: &'static str,
    },
    Decide {
        slug: &'static str,
        expected_hash: &'a str,
        decision: &'static str,
        reason: &'static str,
        request_id: &'static str,
    },
    Change {
        slug: &'static str,
        expected_hash: &'a str,
        request_id: &'static str,
    },
}
#[derive(Serialize)]
pub(super) struct ConsolidationOperation {
    pub request: Value,
    pub request_id: Value,
    pub run: Value,
}
