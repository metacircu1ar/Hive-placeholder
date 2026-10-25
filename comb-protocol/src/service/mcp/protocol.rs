//! Typed JSON-RPC envelope, preserving Value-first duplicate-key handling.
use serde::{Deserialize, Serialize};
use serde_json::Value;

// Object-frame IDs are checked before Frame decoding. Serde also accepts
// positional frames, whose IDs retain their existing opaque JSON shape.
#[derive(Serialize)]
#[serde(transparent)]
pub struct RpcId(Value);

#[derive(Deserialize)]
struct Frame {
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

pub enum Message {
    Request {
        id: RpcId,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
    },
}

pub fn decode(bytes: &[u8]) -> Result<Message, RpcError> {
    // Direct Frame deserialization would reject duplicate keys; Value retains
    // the last value before the envelope is checked, as the protocol requires.
    let raw: Value = serde_json::from_slice(bytes).map_err(|_| RpcError::parse())?;
    let invalid_id = raw.get("id").is_some_and(|id| {
        !id.is_string()
            && !id
                .as_number()
                .is_some_and(|number| number.is_i64() || number.is_u64())
    });
    let frame: Frame = serde_json::from_value(raw).map_err(|_| RpcError::invalid())?;
    if frame.jsonrpc != "2.0" || invalid_id {
        return Err(RpcError::invalid());
    }
    match frame.id {
        Some(id) => Ok(Message::Request {
            id: RpcId(id),
            method: frame.method,
            params: frame.params,
        }),
        None => Ok(Message::Notification {
            method: frame.method,
        }),
    }
}

pub enum Code {
    ParseError,
    InvalidRequest,
    InvalidParams,
    MethodNotFound,
    ServerError,
}
impl Serialize for Code {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_i32(match self {
            Self::ParseError => -32700,
            Self::InvalidRequest => -32600,
            Self::InvalidParams => -32602,
            Self::MethodNotFound => -32601,
            Self::ServerError => -32000,
        })
    }
}

#[derive(Serialize)]
pub struct RpcError {
    pub code: Code,
    pub message: &'static str,
}
impl RpcError {
    fn parse() -> Self {
        Self {
            code: Code::ParseError,
            message: "Invalid JSON.",
        }
    }
    fn invalid() -> Self {
        Self {
            code: Code::InvalidRequest,
            message: "Invalid JSON-RPC request.",
        }
    }
    pub fn frame() -> Self {
        Self {
            code: Code::InvalidRequest,
            message: "Invalid or oversized JSON-RPC frame.",
        }
    }
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum Response<T> {
    Result {
        jsonrpc: &'static str,
        id: RpcId,
        result: T,
    },
    Error {
        jsonrpc: &'static str,
        id: Option<RpcId>,
        error: RpcError,
    },
}
impl<T> Response<T> {
    pub fn reply(id: RpcId, result: Result<T, RpcError>) -> Self {
        match result {
            Ok(result) => Self::Result {
                jsonrpc: "2.0",
                id,
                result,
            },
            Err(error) => Self::error(Some(id), error),
        }
    }
    pub fn error(id: Option<RpcId>, error: RpcError) -> Self {
        Self::Error {
            jsonrpc: "2.0",
            id,
            error,
        }
    }
}

#[cfg(test)]
mod tests;
