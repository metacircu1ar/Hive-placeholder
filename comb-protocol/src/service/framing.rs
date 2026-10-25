pub use super::limits::{MCP_BYTES, REQUEST_BYTES, RESPONSE_BYTES};
use crate::{Error, Result};
use serde::Serialize;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

pub async fn read(
    reader: &mut (impl AsyncBufRead + Unpin),
    limit: usize,
) -> Result<Option<Vec<u8>>> {
    let mut result = Vec::new();
    loop {
        let chunk = reader.fill_buf().await?;
        if chunk.is_empty() {
            return if result.is_empty() {
                Ok(None)
            } else {
                Err(Error::incomplete_frame())
            };
        }
        let end = chunk.iter().position(|b| *b == b'\n');
        let count = end.map_or(chunk.len(), |i| i + 1);
        if result.len() + count > limit {
            return Err(Error::frame_size_limit());
        }
        result.extend_from_slice(&chunk[..count]);
        reader.consume(count);
        if end.is_some() {
            return Ok(Some(result));
        }
    }
}
struct Bounded {
    bytes: Vec<u8>,
    limit: usize,
    overflowed: bool,
}
impl std::io::Write for Bounded {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.overflowed = true;
            return Err(std::io::Error::other("JSON output exceeds its size limit"));
        }
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub fn encode(value: &impl Serialize, limit: usize) -> Result<Vec<u8>> {
    let mut out = Bounded {
        bytes: Vec::new(),
        limit: limit.saturating_sub(1),
        overflowed: false,
    };
    serde_json::to_writer(&mut out, value).map_err(|cause| {
        if out.overflowed {
            Error::output_size_limit()
        } else {
            Error::internal_serialization()
        }
        .caused_by(cause)
    })?;
    out.bytes.push(b'\n');
    Ok(out.bytes)
}
pub async fn write(
    writer: &mut (impl AsyncWrite + Unpin),
    value: &impl Serialize,
    limit: usize,
) -> Result<()> {
    writer.write_all(&encode(value, limit)?).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ErrorCode, error::tests::Unencodable};
    use std::error::Error as _;

    #[test]
    fn output_overflow_is_distinct_from_serializer_failure() {
        let internal = encode(&Unencodable, 1024).unwrap_err();
        assert_eq!(internal.code, ErrorCode::InternalError);
        assert_eq!(
            internal.source().unwrap().to_string(),
            "private serializer failure"
        );
        assert!(
            !serde_json::to_string(&internal)
                .unwrap()
                .contains("private")
        );
        let overflow = encode(&"too large", 4).unwrap_err();
        assert_eq!(overflow.code, ErrorCode::SizeLimit);
        assert!(overflow.source().is_some());
        assert_eq!(encode(&true, 5).unwrap(), b"true\n");
        assert_eq!(encode(&true, 4).unwrap_err().code, ErrorCode::SizeLimit);
    }
}
