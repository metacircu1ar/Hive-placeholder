//! Parser adapters reuse pure library value rules and retain exact spelling.
use comb::{
    Result,
    values::{ContentHash, ImportId, RunName, Slug, Timestamp},
};
use comb_protocol::{limits, search, values::limits::SLUG_BYTES};

fn parsed<T: std::str::FromStr<Err = comb::Error>>(value: &str) -> Result<String> {
    value.parse::<T>()?;
    Ok(value.into())
}

pub(crate) fn slug(value: &str) -> Result<String> {
    parsed::<Slug>(value)
}
pub(crate) fn import_id(value: &str) -> Result<String> {
    parsed::<ImportId>(value)
}
pub(crate) fn hash(value: &str) -> Result<String> {
    parsed::<ContentHash>(value)
}
pub(crate) fn timestamp(value: &str) -> Result<String> {
    parsed::<Timestamp>(value)
}
pub(crate) fn name(value: &str) -> Result<String> {
    parsed::<RunName>(value)
}

pub(crate) fn query(value: &str) -> Result<String> {
    search::query(value)?;
    Ok(value.into())
}

fn filter(value: &str, max_bytes: usize) -> Result<String> {
    search::filter_text(value, max_bytes)?;
    Ok(value.into())
}
pub(crate) fn kind(value: &str) -> Result<String> {
    filter(value, limits::KIND_BYTES)
}
pub(crate) fn tag(value: &str) -> Result<String> {
    filter(value, limits::LABEL_BYTES)
}
pub(crate) fn prefix(value: &str) -> Result<String> {
    filter(value, SLUG_BYTES)
}

pub(super) fn expected(path: &str, id: &str) -> Option<&'static str> {
    Some(match (path, id) {
        (_, "slug" | "note" | "target")
        | ("import", "prefix")
        | ("list" | "links" | "backlinks", "after") => {
            "a slug of 1..240 bytes: slash-separated components of 1..100 lowercase \
                ASCII letters, digits, '-' or '_'"
        }
        (_, "expected_hash" | "note_hash" | "target_hash") => {
            "64 lowercase hexadecimal \
            characters"
        }
        (_, "request_id") => "1..128 UTF-8 bytes without control characters",
        ("import", "abandon") => "a canonical import UUID from import --status",
        ("credential revoke", "id") => "an issued credential UUID",
        ("credential create", "name") | (_, "reviewer_role" | "run") => {
            "1..512 UTF-8 bytes without control characters"
        }
        ("search", "query") => "a nonblank query of at most 4096 UTF-8 bytes",
        ("search", "after" | "before") => "an RFC3339 timestamp with an explicit timezone",
        ("list" | "search", "kind" | "tag") => "1..256 UTF-8 bytes",
        ("list" | "search", "prefix") => "a literal prefix of 1..240 UTF-8 bytes",
        ("search", "limit") => "a decimal integer within 1..100",
        (_, "limit") => "a decimal integer within 1..1000",
        (_, "depth") => "a decimal integer within 0..8",
        (_, "vector_cache_mib") => "a decimal integer within 1..8192",
        ("serve", "log_file_mib") => "a decimal integer within 1..1024",
        ("serve", "log_files") => "a decimal integer within 2..64",
        ("logs", "tail") => "a decimal integer within 1..2000",
        ("logs", "event") => "1..128 ASCII letters, digits, _, ., : or -",
        ("model use-http", "dimensions") => "a decimal integer within 1..4096",
        ("model use-http", "max_tokens") => "a decimal integer within 16..8192, with --tokenizer",
        ("model use-http", "timeout_seconds") => "a decimal integer within 1..120 seconds",
        ("model use-http", "model" | "revision") => "1..4096 UTF-8 bytes",
        ("model use-http", "query_prefix" | "document_prefix") => {
            "at most 1024 UTF-8 bytes (empty allowed)"
        }
        ("model use-http", "url") => {
            "an HTTPS endpoint or HTTP on localhost, 127.0.0.1 or [::1], without \
                user-info, query or fragment; normalized http:URL identity at most 4096 \
                bytes"
        }
        ("model use", "name") => {
            "none, the only accepted name (text search only); use model use-http for embeddings"
        }
        (_, "dimensions" | "max_tokens") => "a nonnegative integer",
        (_, "timeout_seconds") => "a nonnegative integer number of seconds",
        _ => return None,
    })
}
