//! Positive native branching uses the exact fixture audit in a libtest child.
use super::{api_fixture::*, *};
#[test]
fn api_branch_and_model_change_keep_native_context_and_selected_key() {
    for provider in [Provider::Codex, Provider::Claude] {
        let (mut f, _) = runtime_fixture(provider);
        f.start();
        let id = f.spawn("reply: inherited fact");
        let parent = startup_finished(&f, &id);
        let branch = f.ok(json!({"op":"spawn","from_agent":id,"body":"reply: child"}));
        let child = branch["agent_id"].as_str().unwrap();
        let branched = startup_finished(&f, child);
        assert_ne!(branched["thread_id"], parent["thread_id"]);
        assert_eq!(branched["branch_phase"], "bound");
        assert_eq!(branched["execution_key"], parent["execution_key"]);
        let model = if provider == Provider::Codex {
            "gpt-5.6-sol"
        } else {
            "haiku"
        };
        f.ok(
            json!({"op":"set_options","target":child,"options":{"model":model,"effort":"medium"}}),
        );
        let sent = f.ok(json!({"op":"send","target":child,"body":"reply: changed"}));
        let changed = finished(&f, child, &sent);
        assert_eq!(changed["thread_id"], branched["thread_id"]);
        assert_eq!(changed["model"], model);
        assert_eq!(changed["effort"], "medium");
        assert_eq!(changed["execution_key"], branched["execution_key"]);
        f.shutdown();
        tree_private(&f.root);
    }
}
