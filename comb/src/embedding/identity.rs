//! Construction descriptors for opaque stored space identities.
use super::INPUT_BYTES;
use crate::{Result, values::ContentHash};
use serde::{Serialize, Serializer};

const TOKENIZERS: &str = "tokenizers=0.23.2";

pub(super) enum BackendIdentity<'a> {
    Http(&'a reqwest::Url),
}
impl BackendIdentity<'_> {
    pub fn encode(self) -> String {
        match self {
            Self::Http(url) => format!("http:{url}"),
        }
    }
}

pub(super) enum Normalization {
    L2F64AccumulateF32,
}
impl Normalization {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::L2F64AccumulateF32 => "l2-f64-accumulate-f32",
        }
    }
}

pub(super) struct Composition;
impl Serialize for Composition {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!(
            "title-newline-heading-newline-body;utf8;input-limit-{INPUT_BYTES}"
        ))
    }
}
pub(super) struct TokenizerEngine;
impl Serialize for TokenizerEngine {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(TOKENIZERS)
    }
}
#[derive(Serialize)]
pub(super) struct TokenizerPolicy<'a> {
    pub tokenizer_sha256: &'a ContentHash,
    pub max_tokens: usize,
    pub engine: TokenizerEngine,
}
#[derive(Serialize)]
pub(super) struct HttpPreprocessing<'a> {
    pub composition: Composition,
    pub request_dimensions: bool,
    pub tokenizer_preflight: Option<TokenizerPolicy<'a>>,
}

pub(super) fn preprocessing(descriptor: impl Serialize) -> Result<String> {
    // Space fingerprints include this JSON string. Preserve recursive key sorting,
    // explicit nulls, and array order from the original Value construction.
    crate::error::json_text(&crate::error::value(descriptor)?)
}
