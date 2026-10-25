use super::inventory::{
    Row,
    Rule::{Deferred, Reject},
};
const PATH: &str = "Filesystem paths are accepted lexically; ownership, existence and content \
    are checked before use.";
const PREFIX_OVER: &str = match std::str::from_utf8(&[b'a'; 1025]) {
    Ok(text) => text,
    Err(_) => panic!("ASCII test input"),
};
pub(super) const ROWS: &[Row] = &[
    ("@global", "comb socket", Deferred(PATH)),
    ("@global", "run", Reject("")),
    (
        "service install",
        "program environment_file",
        Deferred(PATH),
    ),
    ("service install", "service_path", Reject("")),
    (
        "setup",
        "service_environment embedding_config bridge_file",
        Deferred(PATH),
    ),
    ("setup", "service_path", Reject("")),
    ("setup", "reviewer_role", Reject("")),
    ("model use", "name", Reject("")),
    ("model use-http", "url model revision", Reject("")),
    (
        "model use-http",
        "dimensions max_tokens timeout_seconds",
        Reject("bad"),
    ),
    (
        "model use-http",
        "query_prefix document_prefix",
        Reject(PREFIX_OVER),
    ),
    ("model use-http", "tokenizer", Deferred(PATH)),
    (
        "model use-http",
        "api_key_env",
        Deferred(
            "Environment variable names and credential values are validated by HTTP \
                preparation before connecting.",
        ),
    ),
    ("serve", "embedding_config", Deferred(PATH)),
    ("serve", "vector_cache_mib", Reject("bad")),
    ("serve", "log_file_mib log_files", Reject("bad")),
    ("serve", "log_level", Reject("")),
    ("service", "log_file_mib log_files", Reject("bad")),
    ("service", "log_level", Reject("")),
    ("logs", "tail", Reject("bad")),
    ("logs", "level event", Reject("")),
    ("mcp", "credential", Deferred(PATH)),
    ("credential create", "name reviewer_role", Reject("")),
    ("credential create", "out", Deferred(PATH)),
    ("credential revoke", "id", Reject("bad-uuid")),
    ("init|import", "dir", Deferred(PATH)),
    (
        "get|put|append|promote|decide|rm|links|backlinks|graph",
        "slug",
        Reject(""),
    ),
    ("put", "file", Deferred(PATH)),
    (
        "put|append|promote|decide|rm",
        "expected_hash",
        Reject("bad-hash"),
    ),
    (
        "put|append|capture|promote|consolidate|decide|rm",
        "request_id",
        Reject(""),
    ),
    ("append|capture", "text", Reject("")),
    ("append", "source", Reject("")),
    ("append|consolidate", "summary_file", Deferred(PATH)),
    ("consolidate", "note target", Reject("")),
    ("consolidate", "note_hash target_hash", Reject("bad-hash")),
    ("decide", "decision reason", Reject("")),
    ("list|search", "kind tag prefix", Reject("")),
    ("list|links|backlinks", "after", Reject("")),
    ("list|search|links|backlinks", "limit", Reject("bad")),
    ("search", "query mode after before", Reject("")),
    ("search", "embedding_config", Deferred(PATH)),
    ("embed", "config", Deferred(PATH)),
    ("graph", "depth", Reject("bad")),
    ("import", "prefix", Reject("")),
    ("import", "abandon", Reject("bad-uuid")),
];
