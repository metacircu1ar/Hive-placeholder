//! Startup probes distinguish proven local absence from every ambiguous failure.
use super::{CLIENT_RESPONSE_READ_BYTES, Client, envelope, request, socket_path, unavailable};
use crate::{
    error::{Error, Result},
    runtime::api::Request,
};
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt},
    time::Instant,
};

impl Client<'_> {
    /// One deadline covers connect/write/read. Only the local connect result can
    /// prove absence; a daemon error reply is returned intact for the caller.
    pub async fn startup(&self, deadline: Instant) -> Result<Option<Value>> {
        tokio::time::timeout_at(deadline, self.startup_exchange())
            .await
            .map_err(|cause| {
                Error::daemon_start_timeout(
                    "Daemon observation timed out; inspect its status and stop it before \
                        retrying.",
                )
                .with_source(cause)
            })?
    }

    async fn startup_exchange(&self) -> Result<Option<Value>> {
        let mut stream = match tokio::net::UnixStream::connect(socket_path(self.root)).await {
            Ok(stream) => stream,
            Err(cause) if absent(&cause) => return Ok(None),
            Err(cause) => return Err(unavailable(cause)),
        };
        let mut bytes = request("startup-ping", Request::Ping)
            .to_string()
            .into_bytes();
        bytes.push(b'\n');
        stream
            .write_all(&bytes)
            .await
            .map_err(Error::state_io_from)?;
        let mut line = String::new();
        tokio::io::BufReader::new(stream.take(CLIENT_RESPONSE_READ_BYTES))
            .read_line(&mut line)
            .await
            .map_err(Error::state_io_from)?;
        envelope::parse(&line).map(Some)
    }
}

fn absent(cause: &std::io::Error) -> bool {
    matches!(
        cause.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
