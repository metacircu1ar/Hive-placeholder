//! Poll hosted knowledge calls alongside provider frames and lifecycle controls.
use super::{IncomingFrame, Result, ServerMethod, SessionLoop, outgoing};
use crate::runtime::{api, caller::Caller, comb};
use serde_json::Value;
use std::{future::Future, pin::Pin, task::Poll};

type Reply = comb::Result<Value>;
pub(super) struct Pending {
    id: Value,
    turn: String,
    future: Pin<Box<dyn Future<Output = Reply> + Send>>,
}
pub(super) async fn response(pending: &mut [Pending]) -> (usize, Reply) {
    std::future::poll_fn(|cx| {
        for (index, call) in pending.iter_mut().enumerate() {
            if let Poll::Ready(result) = call.future.as_mut().poll(cx) {
                return Poll::Ready((index, result));
            }
        }
        Poll::Pending
    })
    .await
}
impl SessionLoop<'_> {
    pub(super) async fn start_comb(&mut self, frame: &IncomingFrame) -> Result<bool> {
        let params = frame.params();
        let Some(name) = params
            .raw_tool()
            .as_str()
            .filter(|name| name.starts_with("comb_"))
        else {
            return Ok(false);
        };
        if !frame.has_id() || frame.method() != Some(&ServerMethod::ToolCall) {
            return Ok(false);
        }
        let matching = self.active.as_ref().is_some_and(|active| {
            params.thread_id() == Some(self.thread_id) && params.turn_id() == Some(&active.turn_id)
        });
        if !self.rpc.codex() && (self.interrupt_sent || !matching) {
            // The codec may already have retired a callback that was queued to
            // this worker. Never send an unknown-ID reply into the next turn.
            self.rpc.forget_tool(frame.raw_id().clone()).await?;
            return Ok(true);
        }
        let Some(active) = self.active.as_ref().filter(|a| {
            params.thread_id() == Some(self.thread_id) && params.turn_id() == Some(&a.turn_id)
        }) else {
            return Ok(false);
        };
        if self.interrupt_sent || self.pending_comb.len() >= 8 {
            let value = api::value(api::ValueError {
                error: comb::Error::new(
                    "comb_busy",
                    "Agent is interrupting or \
                already has eight pending Comb calls.",
                ),
            });
            self.rpc
                .send(outgoing::tool_reply(frame.raw_id(), &value, false))
                .await?;
            return Ok(true);
        }
        let turn = active.turn_id.clone();
        let native = params
            .call_id()
            .map_or_else(|| frame.raw_id().clone(), api::value);
        let id =
            crate::runtime::tool_registry::request_id(self.execution.execution, &turn, &native);
        let caller = Caller::Agent {
            id: self.execution.id.into(),
            execution: self.execution.execution.into(),
            turn: Some(turn.clone()),
        };
        let shared = self.execution.shared.clone();
        let name = name.to_owned();
        let args = params.arguments().clone();
        self.pending_comb.push(Pending {
            id: frame.raw_id().clone(),
            turn,
            future: Box::pin(async move { shared.comb_call(caller, name, args, id).await }),
        });
        Ok(true)
    }
    pub(super) async fn finish_comb(&mut self, index: usize, result: Reply) -> Result<()> {
        let pending = self.pending_comb.swap_remove(index);
        if self.interrupt_sent
            || self
                .active
                .as_ref()
                .is_none_or(|a| a.turn_id != pending.turn)
        {
            return self.rpc.forget_tool(pending.id).await;
        }
        let success = result.is_ok();
        let value = result.unwrap_or_else(|error| api::value(api::ValueError { error }));
        self.rpc
            .send(outgoing::tool_reply(&pending.id, &value, success))
            .await
    }
    pub(super) async fn abandon_comb(&mut self) -> Result<()> {
        let pending = std::mem::take(&mut self.pending_comb);
        for call in pending {
            let id = call.id.clone();
            drop(call);
            self.rpc.forget_tool(id).await?;
        }
        Ok(())
    }
}
