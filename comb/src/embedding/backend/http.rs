//! Bounded OpenAI-compatible backend and reply validation.
mod transport;

use super::{Space, normalize};
use crate::embedding::{
    HttpConfig, RuntimeInfo, RuntimeProvider,
    config::http::endpoint,
    identity::{
        self, BackendIdentity, Composition, HttpPreprocessing, Normalization, TokenizerEngine,
        TokenizerPolicy,
    },
    limits,
    tokenizer::Preflight,
};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use transport::{Classified, Transport};

#[derive(Clone)]
pub(super) struct HttpBackend {
    client: reqwest::blocking::Client,
    url: reqwest::Url,
    key: Option<String>,
    timeout: Duration,
    request_dimensions: bool,
    pub(super) tokenizer: Option<Arc<Preflight>>,
}
impl HttpBackend {
    pub(super) fn identity(mut config: HttpConfig) -> Result<Space> {
        let url = endpoint(&config.url, config.timeout_seconds)?;
        let tokenizer = preflight(config.tokenizer.take(), config.max_tokens)?;
        let space = specification(config, &url, tokenizer.as_deref())?;
        space.validate()?;
        Ok(space)
    }
    pub(super) fn load(mut config: HttpConfig) -> Result<(Space, Self)> {
        let url = endpoint(&config.url, config.timeout_seconds)?;
        let key = credential(config.api_key_env.take())?;
        let client = client(config.timeout_seconds)?;
        let tokenizer = preflight(config.tokenizer.take(), config.max_tokens)?;
        let timeout = Duration::from_secs(config.timeout_seconds);
        let request_dimensions = config.request_dimensions;
        let space = specification(config, &url, tokenizer.as_deref())?;
        Ok((
            space,
            Self {
                client,
                url,
                key,
                timeout,
                request_dimensions,
                tokenizer,
            },
        ))
    }

    pub(super) fn runtime(&self) -> RuntimeInfo {
        RuntimeInfo {
            provider: RuntimeProvider::Http,
            max_tokens: self.tokenizer.as_ref().map(|p| p.limit),
        }
    }

    pub(super) fn embed(&self, space: &Space, texts: &[&str]) -> Result<Vec<Result<Vec<f32>>>> {
        let mut http = Http {
            client: &self.client,
            url: &self.url,
            key: self.key.as_deref(),
            space,
            request_dimensions: self.request_dimensions,
        };
        transport::embed(
            &mut http,
            texts,
            &space.document_prefix,
            Instant::now() + self.timeout,
        )
    }
}

fn credential(name: Option<String>) -> Result<Option<String>> {
    name.map(|name| {
        std::env::var(name).map_err(|cause| {
            Error::embedding_credential_missing().caused_by(crate::error::Cause::environment(cause))
        })
    })
    .transpose()
}

fn client(timeout_seconds: u64) -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(timeout_seconds))
        .connect_timeout(Duration::from_secs(
            timeout_seconds.min(limits::HTTP_CONNECT_SECONDS),
        ))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|cause| Error::http_transport_initialization().caused_by(cause))
}

fn preflight(path: Option<PathBuf>, max_tokens: Option<usize>) -> Result<Option<Arc<Preflight>>> {
    match (path, max_tokens) {
        (None, None) => Ok(None),
        (Some(path), Some(limit)) => Ok(Some(Arc::new(Preflight::load(&path, limit)?))),
        _ => Err(Error::http_tokenizer_pair_required()),
    }
}

fn specification(
    config: HttpConfig,
    url: &reqwest::Url,
    tokenizer: Option<&Preflight>,
) -> Result<Space> {
    let policy = tokenizer.map(|p| TokenizerPolicy {
        tokenizer_sha256: &p.hash,
        max_tokens: p.limit,
        engine: TokenizerEngine,
    });
    Ok(Space {
        backend: BackendIdentity::Http(url).encode(),
        model: config.model,
        revision: config.revision,
        dimensions: config.dimensions,
        document_prefix: config.document_prefix,
        query_prefix: config.query_prefix,
        preprocessing: identity::preprocessing(HttpPreprocessing {
            composition: Composition,
            request_dimensions: config.request_dimensions,
            tokenizer_preflight: policy,
        })?,
        normalization: Normalization::L2F64AccumulateF32.as_str().into(),
    })
}

struct Http<'a> {
    client: &'a reqwest::blocking::Client,
    url: &'a reqwest::Url,
    key: Option<&'a str>,
    space: &'a Space,
    request_dimensions: bool,
}
impl Transport for Http<'_> {
    fn request(
        &mut self,
        texts: &[&str],
        deadline: Instant,
    ) -> std::result::Result<Vec<Vec<f32>>, Classified> {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(Error::embedding_batch_deadline)?;
        let mut request = self
            .client
            .post(self.url.clone())
            .timeout(remaining)
            .json(&Request {
                model: &self.space.model,
                input: texts,
                encoding_format: "float",
                dimensions: self.request_dimensions.then_some(self.space.dimensions),
            });
        if let Some(key) = self.key {
            request = request.bearer_auth(key);
        }
        let response = request
            .send()
            .map_err(|cause| Error::embedding_request_failed().caused_by(cause))?;
        Classified::status(response.status().as_u16())?;
        if response
            .content_length()
            .is_some_and(|n| n > crate::convert::usize_u64(limits::HTTP_RESPONSE_BYTES))
        {
            return Err(Error::embedding_response_size().into());
        }
        let mut bytes = Vec::new();
        response
            .take(crate::convert::usize_u64(limits::HTTP_RESPONSE_BYTES) + 1)
            .read_to_end(&mut bytes)
            .map_err(|cause| Error::embedding_reply_incomplete().caused_by(cause))?;
        if bytes.len() > limits::HTTP_RESPONSE_BYTES {
            return Err(Error::embedding_response_size().into());
        }
        parse_reply(&bytes, texts.len(), self.space.dimensions).map_err(Classified::Transient)
    }
}
#[derive(Serialize)]
struct Request<'a> {
    model: &'a str,
    input: &'a [&'a str],
    encoding_format: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    dimensions: Option<usize>,
}
#[derive(Deserialize)]
struct Reply {
    data: Vec<Item>,
}
#[derive(Deserialize)]
struct Item {
    index: usize,
    embedding: Vec<f32>,
}
fn parse_reply(bytes: &[u8], count: usize, dimensions: usize) -> Result<Vec<Vec<f32>>> {
    let reply: Reply = serde_json::from_slice(bytes)
        .map_err(|cause| Error::malformed_embedding_response().caused_by(cause))?;
    if reply.data.len() != count {
        return Err(Error::embedding_response_count());
    }
    let mut vectors = vec![None; count];
    for item in reply.data {
        if item.index >= count || vectors[item.index].is_some() {
            return Err(Error::invalid_embedding_indices());
        }
        vectors[item.index] = Some(normalize(item.embedding, dimensions)?);
    }
    vectors
        .into_iter()
        .map(|v| v.ok_or_else(Error::embedding_index_missing))
        .collect()
}
