use super::*;
use serde::{Deserialize, Serialize};
use serde_json::Map;

#[derive(Deserialize, Serialize)]
pub(crate) struct AgentView {
    pub id: String,
    pub state: String,
    pub task_outcome: String,
    pub thread_id: Option<String>,
    pub execution_id: Option<String>,
    #[serde(flatten)]
    other: Map<String, Value>,
}
#[derive(Deserialize)]
struct StatusView {
    agent: AgentView,
}
#[derive(Deserialize, Serialize)]
pub(crate) struct EventView {
    pub seq: u64,
    pub agent_id: String,
    pub execution_id: Option<String>,
    pub timestamp: u64,
    #[serde(rename = "type")]
    pub kind: String,
    pub data: Value,
}
#[derive(Deserialize, Serialize)]
pub(crate) struct EventsView {
    pub events: Vec<EventView>,
    pub cursor: u64,
    pub pruned_through: u64,
    pub history_expired: bool,
    #[serde(flatten)]
    other: Map<String, Value>,
}
#[derive(Deserialize, Serialize)]
pub(crate) struct MessageView {
    pub seq: u64,
    pub id: String,
    pub sender: String,
    pub recipient: String,
    pub body: String,
    pub state: String,
    #[serde(flatten)]
    other: Map<String, Value>,
}
#[derive(Deserialize, Serialize)]
pub(crate) struct MessagesView {
    pub messages: Vec<MessageView>,
    pub next_before: Option<u64>,
    #[serde(flatten)]
    other: Map<String, Value>,
}
impl Harness {
    pub(crate) fn status(&self, id: &str) -> Value {
        let response = self.ok(json!({"op":"status","target":id}));
        assert!(response.is_object(), "invalid status envelope: {response}");
        let status: StatusView = serde_json::from_value(response).unwrap();
        serde_json::to_value(status.agent).unwrap()
    }
    pub(crate) fn events(&self, id: &str) -> EventsView {
        let mut after = 0;
        let mut events = Vec::new();
        loop {
            let response = self.ok(json!({"op":"read","target":id,"after":after,"limit":200}));
            assert!(
                response["events"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(Value::is_object),
                "event rows must be objects: {response}"
            );
            let mut page: EventsView = serde_json::from_value(response).unwrap();
            assert!(!page.history_expired, "test history was pruned for {id}");
            if page.events.is_empty() {
                page.events = events;
                return page;
            }
            assert!(page.cursor > after, "event cursor did not advance for {id}");
            after = page.cursor;
            events.append(&mut page.events);
        }
    }
    pub(crate) fn read(&self, id: &str) -> Value {
        serde_json::to_value(self.events(id)).unwrap()
    }
    pub(crate) fn messages(&self, id: &str) -> MessagesView {
        let mut before = None;
        let mut messages = Vec::new();
        loop {
            let mut args = json!({"op":"messages","target":id,"limit":200});
            if let Some(cursor) = before {
                args["before"] = json!(cursor);
            }
            let mut page: MessagesView = serde_json::from_value(self.ok(args)).unwrap();
            messages.append(&mut page.messages);
            let Some(cursor) = page.next_before else {
                page.messages = messages;
                return page;
            };
            assert!(
                before.is_none_or(|previous| cursor < previous),
                "mailbox cursor did not advance for {id}"
            );
            before = Some(cursor);
        }
    }
    pub(crate) fn mailbox(&self, id: &str) -> Value {
        serde_json::to_value(self.messages(id)).unwrap()
    }
}
