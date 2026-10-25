//! Events for the Model view.
use super::{
    Entry, Event, Item, LIVE_TAIL, Live, Model, Payload, artifact_notice, describe_tool, kind_of,
    notice, sender, tail,
};

impl Model {
    pub(super) fn apply_event(&mut self, event: &Event<'_>) {
        let seq = event.seq;
        let data = &event.data;
        if let Some(notice) = artifact_notice(event) {
            self.push(seq, Entry::Notice(notice));
            return;
        }
        match event.kind.unwrap_or_default() {
            // The spawn task is the agent's first input, from its parent or a human.
            "spawned" => self.push(
                seq,
                Entry::Input {
                    sender: data.parent_id.unwrap_or("human").to_owned(),
                    body: data.body.unwrap_or_default().to_owned(),
                    mode: None,
                },
            ),
            // A redirect is recorded as its own kind with the same payload.
            "message_accepted" | "redirect_accepted" => self.push(
                seq,
                Entry::Input {
                    sender: sender(&data.sender),
                    body: data.body.unwrap_or_default().to_owned(),
                    mode: data.mode.map(str::to_owned),
                },
            ),
            "turn_started" => {
                self.push(
                    seq,
                    Entry::TurnStarted {
                        model: data.model.unwrap_or_default().to_owned(),
                        effort: data.effort.unwrap_or_default().to_owned(),
                    },
                );
            }
            "turn_finished" => {
                self.finish_turn(data.turn_id);
                self.push(
                    seq,
                    Entry::TurnFinished {
                        status: data.status.unwrap_or_default().to_owned(),
                    },
                );
            }
            // A turn that ends without `turn_finished` closes all live entries.
            "execution_stopped" | "execution_failed" | "lifetime_expired" => {
                self.finish_turn(None);
                if let Some(text) = notice(kind_of(event), data) {
                    self.push(seq, Entry::Notice(text));
                }
            }
            "assistant_message" => self.assistant(seq, data),
            "tool_completed" => self.tool_completed(seq, data),
            "approval_requested" => self.push(
                seq,
                Entry::Approval {
                    id: data.approval_id.unwrap_or_default().to_owned(),
                    title: data.title.map(str::to_owned),
                    open: true,
                },
            ),
            "approval_closed" => {
                self.close_approval(seq, data);
            }
            kind => {
                if let Some(text) = notice(kind, data) {
                    self.push(seq, Entry::Notice(text));
                }
            }
        }
    }

    pub(super) fn assistant(&mut self, seq: Option<u64>, data: &Payload<'_>) {
        let key = data.item_id.map(str::to_owned);
        let text = data.text.unwrap_or_default().to_owned();
        let streamed = self.take_live(key.as_deref(), |entry| {
            matches!(entry, Entry::Assistant { .. })
        });
        let streamed = match streamed {
            Some(Entry::Assistant { live, .. }) => live,
            _ => String::new(),
        };
        if data.aggregation == Some("native_message") {
            // The aggregate is the whole message: it replaces its stored blocks.
            let position = self.items.iter().position(
                |item| matches!(&item.entry, Entry::Assistant { key: k, .. } if *k == key),
            );
            self.items.retain(
                |item| !matches!(&item.entry, Entry::Assistant { key: k, .. } if *k == key),
            );
            self.recount();
            let entry = Entry::Assistant {
                key,
                text,
                live: String::new(),
                aggregate: true,
            };
            match position {
                Some(index) if index <= self.items.len() => {
                    self.bytes += entry.bytes();
                    self.items.insert(index, Item { seq, entry });
                }
                _ => self.push(seq, entry),
            }
            return;
        }
        if self.header.claude
            && let Some(Item {
                entry:
                    Entry::Assistant {
                        text: stored, live, ..
                    },
                ..
            }) = self
                .items
                .iter_mut()
                .rev()
                .find(|item| key.is_some() && item.entry.key() == key.as_deref())
        {
            // A later block of a stored Claude message appends to it.
            stored.push_str("\n\n");
            stored.push_str(&text);
            if live.len() <= stored.len() {
                live.clear();
            }
            self.recount();
            return;
        }
        // Codex items are complete; a Claude block keeps longer streamed text.
        let live = if self.header.claude && streamed.len() > text.len() {
            streamed
        } else {
            String::new()
        };
        self.push(
            seq,
            Entry::Assistant {
                key,
                text,
                live,
                aggregate: false,
            },
        );
    }

    pub(super) fn tool_completed(&mut self, seq: Option<u64>, data: &Payload<'_>) {
        let item = &data.item;
        let key = item.id.map(str::to_owned);
        let streamed = self.take_live(key.as_deref(), |entry| matches!(entry, Entry::Tool { .. }));
        let output = match streamed {
            Some(Entry::Tool { output, .. }) => output,
            _ => String::new(),
        };
        let (tool, summary) = describe_tool(item);
        let status = item
            .status
            .map(str::to_owned)
            .or_else(|| item.exit_code.map(|code| format!("exit {code}")));
        let output = item
            .aggregated_output
            .map_or(output, |stored| tail(stored, LIVE_TAIL).to_owned());
        self.push(
            seq,
            Entry::Tool {
                key,
                tool,
                summary,
                status,
                output,
                running: false,
            },
        );
    }

    /// Remove and return a live entry of this key and shape.
    pub(super) fn take_live(
        &mut self,
        key: Option<&str>,
        shape: impl Fn(&Entry) -> bool,
    ) -> Option<Entry> {
        let key = key?;
        let index = self
            .live
            .iter()
            .position(|live| shape(&live.entry) && live.entry.key() == Some(key))?;
        Some(self.live.remove(index).entry)
    }

    /// A finished turn closes its open tools; its live entries that never became
    /// durable (MCP tools, interrupted text) stay in the transcript. A later
    /// turn's live entries keep streaming.
    pub(super) fn finish_turn(&mut self, turn: Option<&str>) {
        let (finished, kept): (Vec<Live>, Vec<Live>) = std::mem::take(&mut self.live)
            .into_iter()
            .partition(|live| {
                turn.is_none() || live.turn.is_none() || live.turn.as_deref() == turn
            });
        self.live = kept;
        self.streaming = !self.live.is_empty();
        for Live { mut entry, .. } in finished {
            match &mut entry {
                Entry::Tool { running, .. } => *running = false,
                Entry::Reasoning { .. } => continue,
                _ => {}
            }
            self.push(None, entry);
        }
        for item in &mut self.items {
            if let Entry::Assistant { text, live, .. } = &mut item.entry
                && !live.is_empty()
                && text.is_empty()
            {
                std::mem::swap(text, live);
            }
        }
    }

    pub(super) fn recount(&mut self) {
        self.bytes = self.items.iter().map(|item| item.entry.bytes()).sum();
    }
}

impl Model {
    fn close_approval(&mut self, seq: Option<u64>, data: &Payload<'_>) {
        let id = data.approval_id.unwrap_or_default().to_owned();
        for item in &mut self.items {
            if let Entry::Approval {
                id: open_id, open, ..
            } = &mut item.entry
                && *open_id == id
            {
                *open = false;
            }
        }
        let action = data.action.unwrap_or("closed");
        self.push(seq, Entry::Notice(format!("approval {action}")));
    }
}
