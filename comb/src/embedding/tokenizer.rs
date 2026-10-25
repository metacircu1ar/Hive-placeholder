//! Optional HTTP token preflight, independent of the ONNX feature.
use super::{backend::read_bounded, limits};
use crate::{Error, Result, values::ContentHash};
use std::path::Path;

pub(super) struct Preflight {
    tokenizer: tokenizers::Tokenizer,
    pub limit: usize,
    pub hash: ContentHash,
}
impl Preflight {
    pub fn load(path: &Path, limit: usize) -> Result<Self> {
        if !(limits::TOKENS_MIN..=limits::TOKENS_MAX).contains(&limit) {
            return Err(Error::http_tokenizer_token_limit());
        }
        let bytes = read_bounded(path, limits::TOKENIZER_BYTES)?;
        let mut tokenizer =
            tokenizers::Tokenizer::from_bytes(&bytes).map_err(Error::http_tokenizer_load)?;
        tokenizer
            .with_truncation(None)
            .map_err(Error::http_tokenizer_truncation)?;
        tokenizer.with_padding(None);
        Ok(Self {
            tokenizer,
            limit,
            hash: ContentHash::digest(bytes),
        })
    }
    pub fn accepts(&self, text: &str) -> Result<bool> {
        let count = self
            .tokenizer
            .encode(text, true)
            .map_err(Error::http_tokenizer_encode)?
            .len();
        Ok(count <= self.limit)
    }
}
