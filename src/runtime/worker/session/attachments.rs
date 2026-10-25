//! Keep attachment preparation outside the writer and final decisions execution-fenced.
use super::{Error, Message, Result, SessionLoop, outgoing};
use crate::runtime::attachments::delivery;

impl SessionLoop<'_> {
    pub(super) async fn prepare_attachments(
        &mut self,
        message: Message,
        selection: &super::OptionsSelection,
    ) -> Result<delivery::Prepared> {
        let summary = if self.rpc.codex() {
            selection
                .options
                .string("codex.reasoning_summary")
                .or_else(|| {
                    self.applied_turn_selection
                        .options
                        .values
                        .contains_key("codex.reasoning_summary")
                        .then_some("auto")
                })
        } else {
            None
        };
        let request = outgoing::turn_start(
            self.thread_id,
            &selection.model,
            &selection.effort,
            summary,
            &message.input(),
        );
        if message.attachments.is_empty() {
            return Ok(delivery::Prepared { message, request });
        }
        let outgoing::OutgoingRequest::TurnStart(turn) = request else {
            unreachable!()
        };
        let codex = self.rpc.codex();
        let prepared = tokio::task::spawn_blocking(move || {
            delivery::prepare(message, turn, codex, crate::limits::JOIN_TRANSPORT_BYTES)
        })
        .await
        .map_err(|e| Error::state_io("Attachment preparation did not finish.").with_source(e))??;
        let items = prepared.message.attachments.clone();
        let id = prepared.message.id.clone();
        self.execution
            .agent(move |core, agent| {
                if agent.reserved_turn.as_ref().map(|r| r.message_id.as_str()) != Some(id.as_str())
                {
                    return Err(Error::stale_execution(
                        "Attachment message reservation no longer matches.",
                    ));
                }
                core.store()
                    .save_claimed_attachment_delivery(&agent.id, &id, &items)
            })
            .await?;
        Ok(prepared)
    }
}
