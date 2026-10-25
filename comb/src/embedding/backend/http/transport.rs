//! Input isolation over one absolute-deadline transport; no retry scheduling.
use crate::{Error, Result};
use std::time::Instant;

#[derive(Debug)]
pub(super) enum Classified {
    // Whole-batch failures remain pending for the worker's existing backoff.
    // This includes invalid replies, not just retryable network/status errors.
    Transient(Error),
    InputRejected(Error),
    EndpointRejected(Error),
}
impl Classified {
    pub(super) fn status(status: u16) -> std::result::Result<(), Self> {
        match status {
            400 | 413 | 422 => Err(Self::InputRejected(Error::embedding_input_rejected(status))),
            200..=299 => Ok(()),
            _ => Err(Self::Transient(Error::embedding_http_status(status))),
        }
    }

    fn failed_probe(self) -> Self {
        match self {
            Self::InputRejected(error) => {
                Self::EndpointRejected(Error::embedding_endpoint_rejected().caused_by(error))
            }
            error => error,
        }
    }

    fn into_error(self) -> Error {
        match self {
            Self::Transient(error) | Self::InputRejected(error) | Self::EndpointRejected(error) => {
                error
            }
        }
    }
}
impl From<Error> for Classified {
    fn from(error: Error) -> Self {
        Self::Transient(error)
    }
}

pub(super) trait Transport {
    fn request(
        &mut self,
        texts: &[&str],
        deadline: Instant,
    ) -> std::result::Result<Vec<Vec<f32>>, Classified>;
}

pub(super) fn embed(
    transport: &mut impl Transport,
    texts: &[&str],
    document_prefix: &str,
    deadline: Instant,
) -> Result<Vec<Result<Vec<f32>>>> {
    isolate(transport, texts, document_prefix, deadline, &mut false).map_err(Classified::into_error)
}

fn isolate(
    transport: &mut impl Transport,
    texts: &[&str],
    document_prefix: &str,
    deadline: Instant,
    probe_ok: &mut bool,
) -> std::result::Result<Vec<Result<Vec<f32>>>, Classified> {
    match transport.request(texts, deadline) {
        Ok(vectors) => Ok(vectors.into_iter().map(Ok).collect()),
        Err(Classified::InputRejected(error)) => {
            if !*probe_ok {
                let probe = format!("{document_prefix}test");
                transport
                    .request(&[&probe], deadline)
                    .map_err(Classified::failed_probe)?;
                *probe_ok = true;
            }
            if texts.len() == 1 {
                return Ok(vec![Err(error)]);
            }
            let (left, right) = texts.split_at(texts.len() / 2);
            let mut results = isolate(transport, left, document_prefix, deadline, probe_ok)?;
            results.extend(isolate(
                transport,
                right,
                document_prefix,
                deadline,
                probe_ok,
            )?);
            Ok(results)
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests;
