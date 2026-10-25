use super::*;

#[test]
fn valid_event_payloads_remain_borrowed() {
    use serde_json::json;
    let events = json!({"events":[{"seq":1,"agent_id":"a","execution_id":null,
        "timestamp":1,"type":"turn_finished","data":{"opaque":[null,2]}}],
        "cursor":1,"pruned_through":0,"history_expired":false});
    let reply = DaemonReply::new(&events).expect("current response");
    let page = reply.events().expect("current events");
    let event = page.first_turn_finished().expect("literal event matches");
    assert!(std::ptr::eq(event, &raw const events["events"][0]));
    assert!(std::ptr::eq(
        page.pruned_through(),
        &raw const events["pruned_through"]
    ));
}

fn follow_page() -> Value {
    serde_json::json!({"events":[
        {"seq":11,"agent_id":"a","execution_id":null,"timestamp":1,
         "type":"turn_finished","data":{"opaque":[null,2]}},
        {"seq":17,"agent_id":"a","execution_id":"execution","timestamp":2,
         "type":"turn_finished","data":{}}
    ],"cursor":17,"pruned_through":10,"history_expired":true})
}

#[test]
fn follow_validates_global_sequence_gaps_and_compaction_frontier() {
    let raw = follow_page();
    let reply = DaemonReply::new(&raw).unwrap();
    let page = reply.events().unwrap();
    page.validate_follow("a", 5, 2).unwrap();
    assert!(!page.is_empty());
    for (after, pruned, cursor, expired) in [(5, 10, 10, true), (18, 10, 18, false)] {
        let raw = serde_json::json!({"events":[],"cursor":cursor,
            "pruned_through":pruned,"history_expired":expired});
        let reply = DaemonReply::new(&raw).unwrap();
        let page = reply.events().unwrap();
        page.validate_follow("a", after, 1).unwrap();
        assert!(page.is_empty());
    }
}

#[test]
fn follow_rejects_inconsistent_pages_before_advancing() {
    use serde_json::json;
    let mutations = [
        ("/events/0/seq", json!(-1)),
        ("/events/0/seq", json!(0)),
        ("/events/0/seq", json!(5)),
        ("/events/0/seq", json!(10)),
        ("/events/0/agent_id", json!("other")),
        ("/events/1/seq", json!(11)),
        ("/events/1/seq", json!(9)),
        ("/cursor", json!(4)),
        ("/cursor", json!(16)),
        ("/cursor", json!(18)),
        ("/pruned_through", json!(18)),
        ("/history_expired", json!(false)),
    ];
    for (pointer, replacement) in mutations {
        let mut raw = follow_page();
        *raw.pointer_mut(pointer).unwrap() = replacement;
        let reply = DaemonReply::new(&raw).unwrap();
        assert_eq!(
            reply
                .events()
                .unwrap()
                .validate_follow("a", 5, 2)
                .unwrap_err()
                .code(),
            "daemon_protocol_error",
            "{raw}"
        );
    }
    let raw = follow_page();
    let reply = DaemonReply::new(&raw).unwrap();
    assert!(reply.events().unwrap().validate_follow("a", 5, 1).is_err());
}
