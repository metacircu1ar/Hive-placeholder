//! Only the read-only catalogue lookup may retry on this connection.
use super::{Duration, Error, REQUEST_TIMEOUT, Response, Result, Rpc, Value, outgoing, protocol};
use crate::runtime::provider::error::ACK_TIMEOUT;
use crate::runtime::provider::protocol::MODEL_PAGE_LIMIT;
use std::time::Instant;

// Shared by every page: a long catalogue cannot multiply the retry budget.
const RETRY_DELAYS: [Duration; 2] = [Duration::from_millis(250), Duration::from_millis(500)];

pub(crate) struct ModelLookup {
    retries: usize,
    deadline: Instant,
}
impl Default for ModelLookup {
    fn default() -> Self {
        Self {
            retries: 0,
            deadline: Instant::now() + REQUEST_TIMEOUT,
        }
    }
}
impl ModelLookup {
    pub(crate) fn page(&mut self, rpc: &mut Rpc, cursor: &Value) -> Result<Response> {
        // Claude's catalogue is a local codec lookup and keeps its existing bound.
        if rpc.claude.is_some() {
            return rpc.call(outgoing::model_page(MODEL_PAGE_LIMIT, cursor));
        }
        self.page_with(rpc, cursor, std::thread::sleep)
    }

    fn page_with(
        &mut self,
        rpc: &mut Rpc,
        cursor: &Value,
        mut delay: impl FnMut(Duration),
    ) -> Result<Response> {
        loop {
            let remaining = self.deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::provider_timeout(ACK_TIMEOUT));
            }
            let result = rpc.call_until(
                outgoing::model_page(MODEL_PAGE_LIMIT, cursor),
                self.deadline,
            );
            // A malformed late reply can poison the transport while the current
            // request succeeds. Never publish a catalogue from that connection.
            if rpc.uncertain() {
                return result.and_then(|_| Err(protocol()));
            }
            match result {
                Err(error)
                    if rpc.claude.is_none()
                        && self.retries < RETRY_DELAYS.len()
                        && matches!(error, Error::ProviderRequestFailed { .. }) =>
                {
                    let wait = RETRY_DELAYS[self.retries];
                    if wait >= self.deadline.saturating_duration_since(Instant::now()) {
                        return Err(error);
                    }
                    self.retries += 1;
                    tracing::warn!(name: "provider.model_list_retry",
                         pid = rpc.pid(),
                         attempt = self.retries,
                         code = error.code(),
                         decision = "retry_read_only_lookup");
                    // Only explicit rejection retries. An unanswered request
                    // keeps its full remaining budget and late-ACK ownership.
                    delay(wait);
                }
                result => return result,
            }
        }
    }
}

#[cfg(test)]
mod tests;
