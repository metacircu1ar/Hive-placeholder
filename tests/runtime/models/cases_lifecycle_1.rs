//! Lifecycle regressions.
use super::*;

#[test]
fn unloaded_codex_resumes_verified_selection_then_changes_the_next_turn() {
    let mut f = Harness::builder(Provider::Codex)
        .scenario(json!({"resume_ignores_selection":true}))
        .build();
    f.start();
    let id = f.spawn("hold");
    let original = f.wait(&id, |a| a["state"] == "running");
    f.ok(json!({"op":"stop","target":id}));
    f.wait(&id, |a| a["state"] == "stopped");
    change(&f, &id, "gpt-5.6-luna", "medium");
    f.ok(json!({"op":"send","target":id,"body":"hold"}));
    f.ok(json!({"op":"resume","target":id}));
    let active = f.wait(&id, |a| a["state"] == "running");
    assert_eq!(active["thread_id"], original["thread_id"]);
    assert_ne!(active["execution_id"], original["execution_id"]);
    assert_eq!(active["session_selection"], original["session_selection"]);
    assert_eq!(active["turn_selection"]["model"], "gpt-5.6-luna");
    assert_eq!(active["turn_selection"]["effort"], "medium");
    let requests: Vec<Value> = fs::read_to_string(f.provider_home.join("turn-selections.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(requests[1]["model"], "gpt-5.6-luna");
    assert_eq!(requests[1]["effort"], "medium");
    f.shutdown();
}

#[test]
fn codex_resume_verifies_known_pairs_and_rejects_unknown_selection() {
    for (persist, model, effort, error) in [
        (false, None, None, None),
        (true, None, None, None),
        (true, Some("gpt-5.6-sol"), None, Some("model_mismatch")),
        (
            true,
            Some("gpt-5.6-terra"),
            Some("medium"),
            Some("model_mismatch"),
        ),
        (
            true,
            Some("gpt-5.6-luna"),
            Some("high"),
            Some("effort_mismatch"),
        ),
    ] {
        check_codex_resume(persist, model, effort, error);
    }
}
