//! Live for the Model view.
use super::{
    Entry, Item, LIVE_LIMIT, LIVE_TAIL, Live, LiveItem, Model, Value, append_tail, tail, views,
};

impl Model {
    /// Apply one live page's items.
    pub(in crate::cli::attach) fn apply_live(&mut self, items: &[Value]) {
        for item in items {
            self.live_item(item);
        }
    }

    pub(super) fn live_item(&mut self, raw: &Value) {
        let item: LiveItem<'_> = views::read(raw);
        let key = item.item_id.map(str::to_owned);
        let turn = item.turn_id.map(str::to_owned);
        let text = item.text.unwrap_or_default();
        match item.kind.unwrap_or_default() {
            "turn_started" => self.streaming = true,
            "turn_completed" => {
                self.streaming = false;
                for live in &mut self.live {
                    if let Entry::Tool { running, .. } = &mut live.entry
                        && (turn.is_none() || live.turn == turn)
                    {
                        *running = false;
                    }
                }
            }
            "text" => self.live_text(key, turn, text),
            "reasoning" => {
                match self.live_entry(key.as_deref(), |entry| {
                    matches!(entry, Entry::Reasoning { .. })
                }) {
                    Some(Entry::Reasoning { text: stored, .. }) => append_tail(stored, text),
                    _ => self.push_live(
                        turn,
                        Entry::Reasoning {
                            key,
                            text: tail(text, LIVE_TAIL).to_owned(),
                        },
                    ),
                }
            }
            "tool_started" => self.tool_started(key, turn, text, &item),
            "tool_output" => {
                if let Some(Entry::Tool { output, .. }) =
                    self.live_entry(key.as_deref(), |entry| matches!(entry, Entry::Tool { .. }))
                {
                    append_tail(output, text);
                }
            }
            "tool_completed" => {
                if let Some(Entry::Tool {
                    running, status, ..
                }) = self.live_entry(key.as_deref(), |entry| matches!(entry, Entry::Tool { .. }))
                {
                    *running = false;
                    *status = item
                        .status
                        .map(str::to_owned)
                        .or_else(|| (!text.is_empty()).then(|| text.to_owned()));
                }
            }
            _ => {}
        }
    }

    pub(super) fn live_text(&mut self, key: Option<String>, turn: Option<String>, text: &str) {
        self.streaming = true;
        // A stored Claude block of this message: stream into its entry.
        if let Some(Item {
            entry: Entry::Assistant {
                text: stored, live, ..
            },
            ..
        }) = self
            .items
            .iter_mut()
            .rev()
            .find(|entry| key.is_some() && entry.entry.key() == key.as_deref())
        {
            if live.is_empty() {
                live.push_str(stored);
            }
            append_tail(live, text);
            return;
        }
        match self.live_entry(key.as_deref(), |entry| {
            matches!(entry, Entry::Assistant { .. })
        }) {
            Some(Entry::Assistant { live, .. }) => append_tail(live, text),
            _ => self.push_live(
                turn,
                Entry::Assistant {
                    key,
                    text: String::new(),
                    live: tail(text, LIVE_TAIL).to_owned(),
                    aggregate: false,
                },
            ),
        }
    }

    /// Add a live entry, dropping the oldest finished one past the limit.
    pub(super) fn push_live(&mut self, turn: Option<String>, entry: Entry) {
        self.live.push(Live { turn, entry });
        if self.live.len() > LIVE_LIMIT {
            let index = self
                .live
                .iter()
                .position(|live| !matches!(live.entry, Entry::Tool { running: true, .. }))
                .unwrap_or(0);
            self.live.remove(index);
        }
    }

    pub(super) fn tool_started(
        &mut self,
        key: Option<String>,
        turn: Option<String>,
        text: &str,
        item: &LiveItem<'_>,
    ) {
        // A tool already stored (after a resync or refresh) is not reopened.
        if key.is_some()
            && self.items.iter().any(|stored| {
                matches!(stored.entry, Entry::Tool { .. }) && stored.entry.key() == key.as_deref()
            })
        {
            return;
        }
        self.push_live(
            turn,
            Entry::Tool {
                key,
                tool: item.tool.unwrap_or("tool").to_owned(),
                summary: text.to_owned(),
                status: None,
                output: String::new(),
                running: true,
            },
        );
    }

    pub(super) fn live_entry(
        &mut self,
        key: Option<&str>,
        shape: impl Fn(&Entry) -> bool,
    ) -> Option<&mut Entry> {
        let key = key?;
        self.live
            .iter_mut()
            .rev()
            .map(|live| &mut live.entry)
            .find(|entry| shape(entry) && entry.key() == Some(key))
    }
}
