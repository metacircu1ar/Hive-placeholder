//! Bounded artifact hydration on the follower/command worker, never the UI thread.
use super::{Outcome, Value};
use crate::cli::observe::{self, Observation};
use crate::cli::views::{self, ArtifactOutput, Event};
#[cfg(test)]
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};

const PAGE_BYTES: u64 = 65_536;
/// Keep queued hydrated pages bounded too; larger payloads retain an output link.
const PAGE_BUDGET: u64 = 512 * 1024;

pub(super) fn expand(
    reply: &mut Value,
    reader: &mut impl FnMut(&Value) -> Outcome,
    cancelled: &AtomicBool,
) {
    let Some(events) = reply["events"]["events"].as_array_mut() else {
        return;
    };
    let mut budget = PAGE_BUDGET;
    for event in events {
        let projected: Event<'_> = views::read(event);
        if !matches!(
            projected.kind,
            Some(
                "spawned"
                    | "message_accepted"
                    | "redirect_accepted"
                    | "assistant_message"
                    | "tool_completed"
            )
        ) {
            continue;
        }
        let reference = &projected.data.artifact;
        let Some(id) = reference.id.filter(|id| uuid::Uuid::parse_str(id).is_ok()) else {
            continue;
        };
        let Some(bytes) = reference
            .bytes
            .filter(|bytes| *bytes > 0 && *bytes <= budget)
        else {
            continue;
        };
        // Charge attempts too: corrupt or disappearing artifacts cannot multiply work.
        budget -= bytes;
        if let Some(data) = payload(id, bytes, reader, cancelled) {
            event["data"] = data;
        }
    }
}

fn payload(
    id: &str,
    bytes: u64,
    reader: &mut impl FnMut(&Value) -> Outcome,
    cancelled: &AtomicBool,
) -> Option<Value> {
    let mut text = String::new();
    let mut offset = 0;
    while offset < bytes && !cancelled.load(Ordering::Acquire) {
        let request = observe::value(Observation::Read {
            target: &format!("artifact:{id}"),
            after: offset,
            limit: PAGE_BYTES,
        });
        let Outcome::Ok(page) = reader(&request) else {
            return None;
        };
        let page: ArtifactOutput<'_> = views::read(&page);
        let chunk = page.text?;
        let next = offset.checked_add(u64::try_from(chunk.len()).ok()?)?;
        if page.artifact_id != Some(id)
            || page.offset != Some(offset)
            || page.bytes != Some(bytes)
            || next <= offset
            || next > bytes
            || next - offset > PAGE_BYTES
            || page.next_offset != Some(next)
            || page.eof != Some(next == bytes)
        {
            return None;
        }
        text.push_str(chunk);
        offset = next;
    }
    if offset != bytes || cancelled.load(Ordering::Acquire) {
        return None;
    }
    serde_json::from_str::<Value>(&text)
        .ok()
        .filter(Value::is_object)
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: &str = "11111111-2222-4333-8444-555555555555";

    fn event(kind: &str, bytes: usize) -> Value {
        json!({"type":kind, "data":{"artifact":{"id":ID,"bytes":bytes}}})
    }

    #[test]
    fn large_event_payloads_load_by_bounded_utf8_pages() {
        for (kind, data) in [
            (
                "message_accepted",
                json!({"sender":"human", "body":"é".repeat(40_000)}),
            ),
            (
                "assistant_message",
                json!({"item_id":"a", "text":"answer ".repeat(10_000)}),
            ),
            (
                "tool_completed",
                json!({"item":{"id":"tool", "output":"x".repeat(80_000)}}),
            ),
        ] {
            let text = serde_json::to_string(&data).unwrap();
            let mut reply = json!({"events":{"events":[event(kind, text.len())]}});
            let mut offsets = Vec::new();
            expand(
                &mut reply,
                &mut |request| {
                    assert_eq!(request["target"], format!("artifact:{ID}"));
                    let offset = usize::try_from(request["after"].as_u64().unwrap()).unwrap();
                    offsets.push(offset);
                    let mut end = (offset + usize::try_from(PAGE_BYTES).unwrap()).min(text.len());
                    while !text.is_char_boundary(end) {
                        end -= 1;
                    }
                    Outcome::Ok(json!({"artifact_id":ID, "offset":offset,
                    "text":&text[offset..end], "next_offset":end,
                    "bytes":text.len(), "eof":end==text.len()}))
                },
                &AtomicBool::new(false),
            );
            assert!(offsets.len() >= 2);
            assert_eq!(reply["events"]["events"][0]["data"], data);
        }
    }

    #[test]
    fn rejected_cancelled_or_oversize_reads_keep_an_explicit_reference() {
        let original = json!({"events":{"events":[event("assistant_message", 20_000)]}});
        for cancelled in [false, true] {
            let mut reply = original.clone();
            let mut calls = 0;
            expand(
                &mut reply,
                &mut |_| {
                    calls += 1;
                    Outcome::Transport
                },
                &AtomicBool::new(cancelled),
            );
            assert_eq!(reply, original);
            assert_eq!(calls, usize::from(!cancelled));
        }
        let mut large = json!({"events":{"events":[event("assistant_message", usize::try_from(PAGE_BUDGET+1).unwrap())]}});
        let before = large.clone();
        expand(
            &mut large,
            &mut |_| panic!("over-budget artifact was read"),
            &AtomicBool::new(false),
        );
        assert_eq!(large, before);
    }
}
