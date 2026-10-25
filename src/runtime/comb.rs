//! Hosted Comb tools: bounded asynchronous I/O, independent of the runtime writer.
use super::{caller::Caller, hub::Hub};
use crate::config::CombBinding;
use comb_protocol::service::{
    Operation,
    auth::{Claim, Connection},
    client, mcp,
};
use serde::Serialize;
use serde_json::Value;
use std::{borrow::Cow, time::Duration};

/// Hosted replies combine Comb failures with daemon admission and lifecycle errors.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(super) enum Error {
    Comb(comb_protocol::Error),
    Native(NativeError),
}
#[derive(Debug, Serialize)]
pub(super) struct NativeError {
    code: &'static str,
    message: Cow<'static, str>,
    file_committed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<String>,
}
pub(super) type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub(super) fn new(code: &'static str, message: impl Into<Cow<'static, str>>) -> Self {
        Self::Native(NativeError {
            code,
            message: message.into(),
            file_committed: false,
            request_id: None,
        })
    }
    fn set_uncertain(&mut self, op: &Operation) {
        match self {
            Self::Native(error) => {
                error.request_id = op.request_id().map(str::to_owned);
                error.file_committed = op.request_id().is_some();
            }
            Self::Comb(error) => {
                error.request_id = op.request_id().map(str::to_owned);
                error.file_committed = op.request_id().is_some();
            }
        }
    }
}
impl From<comb_protocol::Error> for Error {
    fn from(error: comb_protocol::Error) -> Self {
        Self::Comb(error)
    }
}

pub(super) struct Attestation {
    pub tool_catalog: crate::compatibility::AgentCatalog,
    pub binding: CombBinding,
    pub claim: Claim,
}

fn runtime_error(error: crate::error::Error) -> Error {
    crate::observability::observation_failed("hosted_comb_runtime", &error);
    Error::new(error.code(), error.message().to_owned())
}
impl Hub {
    async fn comb_attestation(&self, caller: Caller) -> Result<Attestation> {
        self.reader
            .get()
            .ok_or_else(|| Error::new("daemon_stopping", "Comb identity reader is unavailable."))?
            .comb(caller)
            .await
            .map_err(runtime_error)
    }
    pub(super) async fn comb_call(
        &self,
        caller: Caller,
        name: String,
        args: Value,
        id: String,
    ) -> Result<Value> {
        let permit = self
            .comb_slots
            .clone()
            .try_acquire_owned()
            .map_err(|cause| {
                crate::observability::observation_failed("hosted_comb_capacity", &cause);
                Error::new(
                    "comb_busy",
                    "The daemon's 64 hosted Comb calls are occupied.",
                )
            })?;
        let op = mcp::operation(&name, args, Some(&id))?;
        let result = tokio::time::timeout(CALL_TIMEOUT, async {
            let attestation = self.comb_attestation(caller.clone()).await?;
            if !super::tool_registry::advertised(
                attestation.tool_catalog,
                &name,
                Some(attestation.binding.reviewer),
            ) {
                return Err(Error::new(
                    "forbidden",
                    "This agent's saved Comb capability does \
                    not allow this tool.",
                ));
            }
            let (connection, _permit) = tokio::task::spawn_blocking(move || {
                Connection::read(&attestation.binding.credential_file)
                    .map(|connection| (connection, permit))
            })
            .await
            .map_err(|cause| {
                let task_id = cause.id().to_string();
                tracing::warn!(name: "comb.binding_worker_failed",
                         task_id = task_id,
                         error = &cause as &(dyn std::error::Error + 'static),
                         decision = "reject_call");
                Error::new("worker_failed", "Could not read the Comb binding.")
            })??;
            // Recheck ownership after file I/O and immediately before submission.
            let current = self.comb_attestation(caller).await?;
            client::call(&connection, &op, Some(current.claim), CALL_TIMEOUT)
                .await
                .map_err(|error| Error::from(mcp::tool_error(&name, error)))
        })
        .await
        .unwrap_or_else(|cause| {
            crate::observability::observation_failed("hosted_comb_deadline", &cause);
            let mut error = Error::new(
                if op.request_id().is_some() {
                    "uncertain"
                } else {
                    "timeout"
                },
                "Comb call exceeded 30 seconds. Accepted work is not cancelled; reuse the \
                    request ID after uncertainty.",
            );
            error.set_uncertain(&op);
            Err(error)
        });
        bounded_reply(result, &op)
    }
}
fn bounded_reply(result: Result<Value>, op: &Operation) -> Result<Value> {
    // Provider envelopes stringify the value, and Claude wraps it again. Eight
    // MiB here leaves ample room for worst-case escaping within the 64 MiB frame.
    if result
        .as_ref()
        .is_ok_and(|value| value.to_string().len() > 8 * 1024 * 1024)
    {
        let mut error = Error::new(
            "output_limit",
            "Comb result exceeds the hosted tool limit; use the Comb CLI to inspect it.",
        );
        error.set_uncertain(op);
        Err(error)
    } else {
        result
    }
}

const CALL_TIMEOUT: std::time::Duration = Duration::from_secs(30);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::tool_registry::{request_id, tools};
    use serde_json::json;
    #[test]
    fn hosted_error_preserves_comb_json() {
        assert_eq!(
            serde_json::to_string(&Error::new("comb_busy", "Hosted calls are occupied.")).unwrap(),
            r#"{"code":"comb_busy","message":"Hosted calls are occupied.","file_committed":false}"#
        );
        for file_committed in [false, true] {
            for request_id in [None, Some("request-id")] {
                let error: comb_protocol::Error = serde_json::from_value(json!({
                    "code": "forbidden",
                    "message": "A Comb error with \"quoted\" text.",
                    "file_committed": file_committed,
                    "request_id": request_id,
                }))
                .unwrap();
                let expected = serde_json::to_string(&error).unwrap();
                assert_eq!(
                    serde_json::to_string(&Error::from(error)).unwrap(),
                    expected
                );
            }
        }
    }
    #[test]
    fn hosted_comb_catalog_golden() {
        for reviewer in [false, true] {
            let catalog = tools(hive_input::compatibility::Exact, Some(reviewer));
            let comb_tools = catalog
                .as_array()
                .unwrap()
                .iter()
                .filter(|tool| tool["name"].as_str().unwrap().starts_with("comb_"))
                .collect::<Vec<_>>();
            let bytes = serde_json::to_vec_pretty(&comb_tools).unwrap();
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
                "comb/tests/fixtures/contracts/hosted-catalog-{reviewer}.json"
            ));
            if std::env::var_os("COMB_RECORD_GOLDENS").is_some() {
                std::fs::write(&path, &bytes).unwrap();
            }
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
    }
    #[test]
    fn request_identity_includes_execution_turn_and_original_native_request() {
        let native = json!(["control", 42]);
        let id = request_id("execution", "turn", &native);
        assert_eq!(id, request_id("execution", "turn", &native));
        for other in [
            request_id("new-execution", "turn", &native),
            request_id("execution", "new-turn", &native),
            request_id("execution", "turn", &json!(["control", 43])),
        ] {
            assert_ne!(id, other);
        }
    }
}
