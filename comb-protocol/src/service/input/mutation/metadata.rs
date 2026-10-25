use crate::Error;
use crate::service::input::samples::{self, ConsolidationOperation, WriteExample};
use hive_input::{Code, InputError};
use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Write {
    Put,
    Append,
    Capture,
    Consolidate,
    Decide,
    Promote,
    Remove,
}
impl Write {
    pub fn named(name: &str) -> Option<Self> {
        match name {
            "put" => Some(Self::Put),
            "append" => Some(Self::Append),
            "capture" => Some(Self::Capture),
            "consolidate" => Some(Self::Consolidate),
            "decide" => Some(Self::Decide),
            "promote" => Some(Self::Promote),
            "remove" => Some(Self::Remove),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Put => "put",
            Self::Append => "append",
            Self::Capture => "capture",
            Self::Consolidate => "consolidate",
            Self::Decide => "decide",
            Self::Promote => "promote",
            Self::Remove => "remove",
        }
    }
    pub fn fields(self) -> &'static [&'static str] {
        match self {
            Self::Put => &["slug", "markdown", "expected_hash", "request_id", "run"],
            Self::Append => &[
                "slug",
                "text",
                "source",
                "summary",
                "expected_hash",
                "request_id",
                "run",
            ],
            Self::Capture => &["text", "request_id", "run"],
            Self::Consolidate => &[
                "note",
                "target",
                "note_hash",
                "target_hash",
                "summary",
                "recopy",
                "request_id",
                "run",
            ],
            Self::Decide => &[
                "slug",
                "expected_hash",
                "decision",
                "reason",
                "request_id",
                "run",
            ],
            Self::Promote => &["slug", "expected_hash", "request_id", "run"],
            Self::Remove => &["slug", "expected_hash", "request_id"],
        }
    }
    pub fn nullable(self, field: &str) -> bool {
        match field {
            "expected_hash" => matches!(self, Self::Put | Self::Append),
            "summary" => self == Self::Append,
            "run" => self != Self::Consolidate && self != Self::Remove,
            _ => false,
        }
    }
    pub fn required(self, field: &str, tool: bool) -> bool {
        !(self.nullable(field) || field == "recopy" || (tool && field == "request_id"))
    }
    pub fn expected(self, field: &str) -> &'static str {
        match field {
            "slug" if self == Self::Decide => "a descendant of review/",
            "note" if self == Self::Consolidate => "a descendant of inbox/",
            "target" if self == Self::Consolidate => "a target outside inbox/ and review/",
            "slug" | "note" | "target" => super::super::retrieval::Read::Get.expected("slug"),
            "expected_hash" | "note_hash" | "target_hash" => {
                "64 lowercase hexadecimal characters from the current page hash"
            }
            "request_id" => {
                "a request ID of 1..128 UTF-8 bytes without control characters; reuse \
                    identical arguments and ID on retries"
            }
            "run" => "a run name of 1..512 UTF-8 bytes without control characters",
            "markdown" => {
                "a complete Comb Markdown page of at most 1048576 UTF-8 bytes without NUL, \
                    with valid title frontmatter and timeline; preserve existing evidence on \
                    replacement"
            }
            "text" if self == Self::Capture => {
                "nonblank authored text of at most 524288 UTF-8 bytes without NUL; any \
                    Markdown timeline must contain valid evidence entries"
            }
            "text" => "nonblank authored text of at most 524288 UTF-8 bytes",
            "source" => "a nonblank evidence source of at most 4096 UTF-8 bytes",
            "summary" if self == Self::Append => {
                "a summary string; supply expected_hash when updating it"
            }
            "summary" => "a nonblank consolidated summary string",
            "reason" => "a nonblank decision reason of at most 4096 UTF-8 bytes",
            "decision" => "accept or reject",
            "recopy" => "a boolean; true explicitly recopies a rewritten note in full",
            "request" => "a consolidation request object",
            _ => "only the documented fields, each occurring once, in this object",
        }
    }
    pub fn sample(self) -> Value {
        let hash = "a".repeat(64);
        match self {
            Self::Put => samples::value(WriteExample::Put {
                slug: "concepts/architecture",
                markdown: "---\ntitle: Architecture\n---\nProject architecture.",
                request_id: "write-1",
            }),
            Self::Append => samples::value(WriteExample::Append {
                slug: "concepts/architecture",
                text: "Verified the boundary.",
                source: "review",
                request_id: "write-1",
            }),
            Self::Capture => samples::value(WriteExample::Capture {
                text: "Investigate the cache boundary.",
                request_id: "write-1",
            }),
            Self::Consolidate => samples::value(WriteExample::Consolidate {
                note: "inbox/source",
                target: "concepts/architecture",
                note_hash: &hash,
                target_hash: &hash,
                summary: "Updated summary.",
                run: "review",
                request_id: "write-1",
            }),
            Self::Decide => samples::value(WriteExample::Decide {
                slug: "review/proposal",
                expected_hash: &hash,
                decision: "accept",
                reason: "Evidence supports the proposal.",
                request_id: "write-1",
            }),
            Self::Promote | Self::Remove => samples::value(WriteExample::Change {
                slug: "concepts/architecture",
                expected_hash: &hash,
                request_id: "write-1",
            }),
        }
    }
    pub fn example(self, tool: bool) -> String {
        let mut sample = self.sample();
        if tool {
            return format!("comb_{} {sample}", self.name());
        }
        if self == Self::Consolidate {
            let id = sample["request_id"].take();
            let run = sample["run"].take();
            if let Some(fields) = sample.as_object_mut() {
                fields.remove("request_id");
                fields.remove("run");
            }
            sample = samples::value(ConsolidationOperation {
                request: sample,
                request_id: id,
                run,
            });
        }
        sample["op"] = self.name().into();
        format!("operation: {sample}")
    }
    pub fn pointer(self, field: &str, tool: bool) -> String {
        let prefix = if tool { "/arguments" } else { "/operation" };
        if field.is_empty() {
            return prefix.into();
        }
        let nested = !tool
            && self == Self::Consolidate
            && !matches!(field, "request" | "request_id" | "run");
        if nested {
            format!("{prefix}/request/{field}")
        } else {
            format!("{prefix}/{field}")
        }
    }
    pub fn error(self, code: Code, field: &str, tool: bool) -> Error {
        match InputError::new(
            code,
            "Invalid write-operation input.",
            self.pointer(field, tool),
            self.expected(field),
            vec![self.example(tool)],
        ) {
            Ok(input) => input.into(),
            Err(cause) => Error::internal_serialization().caused_by(cause),
        }
    }
}
