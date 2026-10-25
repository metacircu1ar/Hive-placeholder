//! Embedding engine dispatch and bounded OpenAI-compatible HTTP transport.
mod http;
use super::{BATCH_SIZE, Config, Embedder, INPUT_BYTES, Space, normalize};
use crate::{Error, Result};
use http::HttpBackend;
use std::{os::unix::fs::OpenOptionsExt as _, path::Path};

pub struct Engine {
    space: Space,
    backend: HttpBackend,
}
impl Engine {
    /// Resolve HTTP identity without reading credentials or constructing a client.
    pub(crate) fn http_identity(config: super::HttpConfig) -> Result<Space> {
        HttpBackend::identity(config)
    }
    pub fn load(config: Config) -> Result<Self> {
        let Config::Http(config) = config;
        let (space, backend) = HttpBackend::load(config)?;
        space.validate()?;
        tracing::info!(name: "comb.embedding_loaded", provider = "http");
        Ok(Self { space, backend })
    }
}
impl Embedder for Engine {
    fn runtime(&self) -> Option<super::RuntimeInfo> {
        Some(self.backend.runtime())
    }

    fn space(&self) -> &Space {
        &self.space
    }
    fn query_engine(&self) -> Option<Box<dyn Embedder>> {
        Some(Box::new(Self {
            space: self.space.clone(),
            backend: self.backend.clone(),
        }))
    }
    fn embed(&mut self, inputs: &[String]) -> Result<Vec<Result<Vec<f32>>>> {
        if inputs.is_empty() || inputs.len() > BATCH_SIZE {
            return Err(Error::embedding_input_count());
        }
        let mut output = vec![Err(Error::embedding_input_byte_limit()); inputs.len()];
        let mut accepted = Vec::new();
        for (i, input) in inputs.iter().enumerate() {
            if input.len() > INPUT_BYTES {
                continue;
            }
            if let Some(check) = &self.backend.tokenizer
                && !check.accepts(input)?
            {
                output[i] = Err(Error::http_input_too_large(check.limit));
                continue;
            }
            accepted.push(i);
        }
        if accepted.is_empty() {
            return Ok(output);
        }
        let texts = accepted
            .iter()
            .map(|i| inputs[*i].as_str())
            .collect::<Vec<_>>();
        let vectors = self.backend.embed(&self.space, &texts)?;
        if vectors.len() != accepted.len() {
            return Err(Error::embedding_result_count());
        }
        // Validate the entire batch before returning any vector.
        let normalized = vectors
            .into_iter()
            .map(|v| match v {
                Ok(v) => normalize(v, self.space.dimensions).map(Ok),
                Err(e) => Ok(Err(e)),
            })
            .collect::<Result<Vec<_>>>()?;
        for (i, vector) in accepted.into_iter().zip(normalized) {
            output[i] = vector;
        }
        Ok(output)
    }
}
pub(super) fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(Error::invalid_embedding_asset());
    }
    if metadata.len() > crate::convert::usize_u64(limit) {
        return Err(Error::embedding_asset_size(path));
    }
    crate::bounded::bytes(file, limit, || Error::embedding_asset_size(path))
}

#[cfg(test)]
mod assets_tests;

#[cfg(test)]
mod bounded_tests {
    use super::read_bounded;
    use crate::{Error, Result};
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn ordinary_assets_keep_symlink_and_shared_file_access() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let target = directory.path().join("asset");
        let link = directory.path().join("linked-asset");
        std::fs::write(&target, b"abc")?;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o666))?;
        symlink(&target, &link)?;
        assert_eq!(read_bounded(&link, 3)?, b"abc");
        let Err(error) = read_bounded(&link, 2) else {
            panic!("oversized asset must fail");
        };
        let expected = Error::embedding_asset_size(&link);
        assert_eq!(error.code, expected.code);
        assert_eq!(error.message, expected.message);
        Ok(())
    }
}
