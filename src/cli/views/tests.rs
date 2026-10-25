use super::*;
use serde_json::json;

#[test]
fn unavailable_fields_keep_neighboring_agent_metadata_and_counts() {
    let raw = json!({"agent":{"id":"a","name":"named","state":7,
        "profile":null,"auth_decision":false,"ended_at":false},
        "queued":-1,"mail":2,"children":3});
    let reply: Attachment<'_> = read(&raw);
    assert_eq!(reply.agent.id, Some("a"));
    assert_eq!(reply.agent.name, Some("named"));
    assert_eq!(reply.agent.state, None);
    assert_eq!(reply.agent.profile.provider, None);
    assert_eq!(reply.agent.auth_decision.reason, None);
    assert!(reply.agent.ended);
    assert_eq!((reply.queued, reply.mail, reply.children), (0, 2, 3));
}

#[test]
fn non_object_rows_do_not_discard_other_agents_or_events() {
    let raw = json!({"agents":[{"id":"a","name":false},null,["array"],{"id":"b"}],
        "events":{"events":[null,{"seq":2,"type":"turn_finished",
        "data":{"status":"completed","body":42}}],"cursor":2}});
    let tree: Tree<'_> = read(&raw);
    let ids: Vec<_> = tree.agents.iter().filter_map(|agent| agent.id).collect();
    assert_eq!(ids, ["a", "b"]);
    let snapshot: Snapshot<'_> = read(&raw);
    assert_eq!(snapshot.events.cursor, Some(2));
    assert_eq!(snapshot.events.events[1].kind, Some("turn_finished"));
    assert_eq!(snapshot.events.events[1].data.status, Some("completed"));
    assert_eq!(snapshot.events.events[1].data.body, None);
}

#[test]
fn explicit_null_usage_sections_remain_present_but_missing_sections_do_not() {
    let raw = json!([{"auth":"a","credits":null,"extra_usage":null},{"auth":"b"}]);
    let reports: Vec<UsageReport<'_>> = list(&raw);
    assert!(reports[0].credits.is_some());
    assert!(reports[0].extra_usage.is_some());
    assert!(reports[1].credits.is_none());
    assert!(reports[1].extra_usage.is_none());
}
