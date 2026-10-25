use crate::Error;
use crate::service::input::samples::{self, ReadExample, RetrievalOperation};
use hive_input::{Code, InputError};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Read {
    Search,
    Get,
    List,
    Links,
    Backlinks,
    Graph,
}

impl Read {
    pub fn named(name: &str) -> Option<Self> {
        match name {
            "search" => Some(Self::Search),
            "get" => Some(Self::Get),
            "list" => Some(Self::List),
            "links" => Some(Self::Links),
            "backlinks" => Some(Self::Backlinks),
            "graph" => Some(Self::Graph),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Get => "get",
            Self::List => "list",
            Self::Links => "links",
            Self::Backlinks => "backlinks",
            Self::Graph => "graph",
        }
    }
    pub fn fields(self) -> &'static [&'static str] {
        match self {
            Self::Search => &[
                "query", "limit", "kind", "tag", "prefix", "after", "before", "mode",
            ],
            Self::List => &["kind", "tag", "prefix", "unverified", "after", "limit"],
            Self::Links | Self::Backlinks => &["slug", "after", "limit"],
            Self::Get => &["slug"],
            Self::Graph => &["slug", "depth"],
        }
    }
    pub fn sample(self) -> serde_json::Value {
        match self {
            Self::Search => samples::value(ReadExample::Search {
                query: "project architecture",
                mode: "text",
                limit: 10,
            }),
            Self::List => samples::value(ReadExample::List {
                prefix: "concepts/",
                limit: 100,
            }),
            Self::Get => samples::value(ReadExample::Slug {
                slug: "concepts/architecture",
            }),
            Self::Links | Self::Backlinks => samples::value(ReadExample::Links {
                slug: "concepts/architecture",
                limit: 100,
            }),
            Self::Graph => samples::value(ReadExample::Graph {
                slug: "concepts/architecture",
                depth: 2,
            }),
        }
    }
    pub fn example(self, tool: bool) -> String {
        let sample = self.sample();
        if tool {
            format!("comb_{} {sample}", self.name())
        } else if matches!(self, Self::Search | Self::List) {
            let mut sample = sample;
            let mode = sample.as_object_mut().and_then(|map| map.remove("mode"));
            let operation = samples::value(RetrievalOperation {
                op: self.name(),
                request: sample,
                mode,
            });
            format!("operation: {operation}")
        } else {
            let mut sample = sample;
            sample["op"] = self.name().into();
            format!("operation: {sample}")
        }
    }
    pub fn expected(self, field: &str) -> &'static str {
        match field {
            "query" => "a nonblank query of at most 4096 UTF-8 bytes",
            "limit" if self == Self::Search => "an integer from 1 through 100",
            "limit" => "an integer from 1 through 1000",
            "depth" => "an integer from 0 through 8",
            "kind" | "tag" => "a nonempty string of at most 256 UTF-8 bytes",
            "prefix" => "a nonempty literal prefix of at most 240 UTF-8 bytes",
            "after" | "before" if self == Self::Search => {
                "an RFC3339 timestamp, with after no later than before"
            }
            "slug" | "after" => {
                "a page slug: lowercase letters, digits, hyphens or underscores in \
                    slash-separated components; at most 240 bytes total and 100 per component"
            }
            "mode" => "text, vector or hybrid",
            "unverified" | "incoming" => "a boolean",
            "request" => "a retrieval request object",
            _ => "only the documented fields in this object",
        }
    }
}

pub fn error(code: Code, field: &str, expected: &str, example: String) -> Error {
    match InputError::new(
        code,
        "Invalid tool or retrieval input.",
        field,
        expected,
        vec![example],
    ) {
        Ok(input) => input.into(),
        Err(cause) => Error::internal_serialization().caused_by(cause),
    }
}
