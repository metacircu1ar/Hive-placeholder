//! Observe provider turn lifecycles while retaining evidence for retired conversations.
use super::{
    EARLY_TURN_LIMIT, IncomingFrame, PROTOCOL_ID_BYTES, ProviderTurnStatus, Result, Rpc,
    ServerMethod, Turn, protocol,
};

impl Rpc {
    pub(super) fn turn(&self, thread: &str) -> Option<&Turn> {
        self.threads
            .get(thread)
            .and_then(|state| state.turn.as_ref())
    }

    pub(super) fn observe(&mut self, value: &IncomingFrame) {
        let Some(method) = value.method() else {
            return;
        };
        let item = method.is_item();
        if !item
            && !matches!(
                method,
                ServerMethod::TurnStarted | ServerMethod::TurnCompleted
            )
        {
            return;
        }
        let Some(thread) = value
            .params()
            .thread_id()
            .filter(|id| !id.is_empty() && id.len() <= PROTOCOL_ID_BYTES)
        else {
            self.protocol_uncertain = true;
            return;
        };
        let turn = if item {
            value.params().turn_id()
        } else {
            value.params().nested_turn_id()
        };
        let Some(turn) = turn.filter(|id| !id.is_empty() && id.len() <= PROTOCOL_ID_BYTES) else {
            self.quarantine(thread);
            return;
        };
        if self.observe_draining_item(value, thread, turn) {
            return;
        }
        self.observe_turn(value, thread, turn, item, method);
    }

    fn observe_turn(
        &mut self,
        value: &IncomingFrame,
        thread: &str,
        turn: &str,
        item: bool,
        method: &ServerMethod,
    ) {
        let Some(state) = self.threads.get_mut(thread) else {
            self.quarantine(thread);
            return;
        };
        match &state.turn {
            Some(Turn::Submitting) if item => {
                let mismatch = state.early_item.as_deref().is_some_and(|id| id != turn);
                state.early_item = Some(turn.into());
                if mismatch {
                    self.quarantine(thread);
                }
            }
            Some(Turn::Submitting) => {
                if state.early.len() < EARLY_TURN_LIMIT {
                    state.early.push(IncomingFrame::early(
                        method,
                        thread,
                        turn,
                        value.params().raw_status(),
                    ));
                } else {
                    self.quarantine(thread);
                }
            }
            Some(Turn::Active(id)) if id == turn => {
                if method == &ServerMethod::TurnCompleted {
                    if value
                        .params()
                        .turn_status()
                        .is_some_and(ProviderTurnStatus::is_terminal)
                    {
                        state.completed = Some(turn.into());
                        state.turn = Some(Turn::Idle);
                        self.complete_drain(thread, turn);
                    } else {
                        self.quarantine(thread);
                    }
                }
            }
            // Retired conversations accept only passive evidence for the exact
            // completed turn; a request or new tool start still fences the owner.
            Some(Turn::Idle) | None
                if state.completed.as_deref() == Some(turn)
                    && !value.has_id()
                    && (method == &ServerMethod::TurnCompleted
                        || (item && method != &ServerMethod::ItemStarted)) => {}
            _ => {
                if state.drained.as_deref() == Some(turn) {
                    state.late_after_drain = true;
                }
                self.quarantine(thread);
            }
        }
    }

    pub(crate) fn thread_uncertain(&self, thread: &str) -> bool {
        self.threads
            .get(thread)
            .is_some_and(|state| state.quarantined)
    }
    pub(crate) fn clean_thread(&self, thread: &str) -> bool {
        !self.protocol_uncertain
            && !self.thread_uncertain(thread)
            && matches!(self.turn(thread), Some(Turn::Idle))
    }
    pub(crate) fn retire_thread(&mut self, thread: &str) -> Result<()> {
        if !self.clean_thread(thread)
            || self
                .threads
                .get(thread)
                .is_some_and(|state| state.drain.is_some())
        {
            return Err(protocol());
        }
        // Retire only the old `threads` lifecycle membership. Completion, early
        // frames/items, terminal-drain history and quarantine evidence stay owned.
        self.threads.get_mut(thread).ok_or_else(protocol)?.turn = None;
        Ok(())
    }
    pub(crate) fn active_turn(&self, thread: &str) -> Result<Option<String>> {
        match self.turn(thread) {
            Some(Turn::Idle) => Ok(None),
            Some(Turn::Active(id)) => Ok(Some(id.clone())),
            _ => Err(protocol()),
        }
    }
}
