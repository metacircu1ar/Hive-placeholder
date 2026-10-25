//! Exact domain spellings shared by JSON, SQLite and operation fingerprints.
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use serde::{Deserialize, Serialize};

macro_rules! domain {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub enum $name {
            $(#[serde(rename = $wire)] $variant),+
        }
        impl $name {
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $wire),+ }
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
        impl ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
                Ok(ToSqlOutput::Borrowed(ValueRef::Text(self.as_str().as_bytes())))
            }
        }
        impl FromSql for $name {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                match value.as_str()? {
                    $($wire => Ok(Self::$variant),)+
                    value => Err(FromSqlError::Other(Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("Unknown {} value: {value}.", stringify!($name)),
                    )))),
                }
            }
        }
    };
}

domain!(Route {
    Text => "text",
    Vector => "vector",
    TextVector => "text+vector",
});
domain!(MatchEvidence {
    KeywordExact => "keyword_exact",
    IdentitySubstring => "identity_substring",
    ExactTitle => "exact_title",
    Alias => "alias",
    SemanticSimilarity => "semantic_similarity",
});
domain!(CreateSafety {
    Probable => "probable",
    Exists => "exists",
    Unknown => "unknown",
});
domain!(Zone {
    Compiled => "compiled",
    Timeline => "timeline",
});
// Variant order preserves lexical ordering in the per-target ordered link set.
domain!(LinkKind {
    Markdown => "markdown",
    Slug => "slug",
});
domain!(FindingKind {
    EmbeddingRejected => "embedding_rejected",
    InvalidPage => "invalid_page",
    PageNearLimit => "page_near_limit",
    IndexStale => "index_stale",
    ExternalEdit => "external_edit",
    UnknownPrefix => "unknown_prefix",
    Orphan => "orphan",
    BrokenLink => "broken_link",
    DanglingCitation => "dangling_citation",
    UncitedParagraph => "uncited_paragraph",
    PrivateStateUnprotected => "private_state_unprotected",
    PrivateStateTracked => "private_state_tracked",
    PrivateStateUnchecked => "private_state_unchecked",
});
// Stored receipt operation claims deliberately remain opaque strings.
domain!(ReceiptOperation {
    Put => "put",
    Append => "append",
    Promote => "promote",
    Remove => "remove",
    Consolidate => "consolidate",
    ConsolidationSource => "consolidation-source",
    Decide => "decide",
});
